//! 最小 Win32 绑定 —— 只声明实际用到的那几个函数。
//!
//! ## 为什么不引 `windows-sys`
//!
//! 本应用只要三件事：一个有名字的互斥体、按可执行文件找回另一个实例的窗口、
//! 等一个进程真的退出。加起来不到十个调用，而 `windows-sys` 会往依赖树里再塞一份
//! feature-gated 的大包。项目里已有的惯例是宁可手写（`main.rs` 的 base64、
//! `open_in_browser` 用 `cmd start` 而不是引 crate），手写还省掉一次联网取 crate
//! —— `Cargo.lock` 因此一行都不用动。
//!
//! ## 失败姿势
//!
//! 三件事各自的失败都**不致命**：宁可少一层保护，也不能让保护本身变成新的启动
//! 故障点（与 `config.rs`「任何读取失败都静默回落到默认值」同一个原则）。

/// 单实例判定的结果。
pub enum Instance {
    /// 本进程是第一个实例；互斥体已持有到进程结束。
    Primary,
    /// 已经有一个实例在跑（互斥体已存在）。
    Duplicate,
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::time::{Duration, Instant};

    use super::Instance;

    /// `Local\` = 每个登录会话一个实例。快速用户切换后两个会话各跑一个是**对的**
    /// （各自有自己的 dsh 后端与窗口）；换成 `Global\` 反而会让两个会话互相挡住。
    const MUTEX_NAME: &str = "Local\\DShell.SingleInstance.v1";

    /// `CreateMutexW` 在名字已存在时返回的 `GetLastError()`。
    const ERROR_ALREADY_EXISTS: u32 = 183;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;
    const WAIT_OBJECT_0: u32 = 0;

    /// 找回窗口的重试间隔：另一个实例可能还在建窗口。
    const FOCUS_RETRY: Duration = Duration::from_millis(150);

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attrs: *mut c_void, owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn GetCurrentProcessId() -> u32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
        fn QueryFullProcessImageNameW(
            process: *mut c_void,
            flags: u32,
            buffer: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            lparam: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
        fn GetWindowTextLengthW(window: *mut c_void) -> i32;
        fn IsWindowVisible(window: *mut c_void) -> i32;
        fn IsIconic(window: *mut c_void) -> i32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
    }

    /// 互斥体句柄：**故意不关**，持有到进程结束 —— 内核在进程退出时释放它，
    /// 也就是"崩溃/被强杀都不会留下一个永远删不掉的锁"。
    static MUTEX: AtomicIsize = AtomicIsize::new(0);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 建命名互斥体来判定是不是第一个实例。
    ///
    /// 建不出来时返回 `Primary`：**不能**因为保护措施失败就拒绝启动。
    pub fn acquire_single_instance() -> Instance {
        let name = wide(MUTEX_NAME);
        let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
        if handle.is_null() {
            crate::log("single instance: CreateMutexW failed - starting without the guard");
            return Instance::Primary;
        }
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        MUTEX.store(handle as isize, Ordering::SeqCst);
        if already_exists {
            Instance::Duplicate
        } else {
            Instance::Primary
        }
    }

    /// 读一个进程的可执行文件全路径；打不开（已退出 / 权限不足）时 `None`。
    unsafe fn image_path(pid: u32) -> Option<String> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let mut buffer = [0u16; 512];
        let mut size = buffer.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle) };
        if ok == 0 || size == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }

    struct PidSearch {
        self_pid: u32,
        self_exe: String,
        found: Option<u32>,
    }

    struct WindowSearch {
        pid: u32,
        visible: Option<*mut c_void>,
        hidden: Option<*mut c_void>,
    }

    /// 找一个**属于其他进程、但 exe 与本进程相同**的窗口，从而定位那个进程。
    unsafe extern "system" fn find_pid(window: *mut c_void, lparam: isize) -> i32 {
        let search = unsafe { &mut *(lparam as *mut PidSearch) };
        if search.found.is_some() {
            return 0;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == 0 || pid == search.self_pid {
            return 1;
        }
        match unsafe { image_path(pid) } {
            Some(path) if path.eq_ignore_ascii_case(&search.self_exe) => {
                search.found = Some(pid);
                0
            }
            _ => 1,
        }
    }

    /// 收集某个进程里有标题的顶层窗口。
    ///
    /// 要求标题非空是为了滤掉 tao / WebView2 的辅助窗口（隐藏的、无标题的）。
    /// 「消息窗口」那种本来就不会被 `EnumWindows` 枚举（它们挂在 `HWND_MESSAGE` 下）。
    unsafe extern "system" fn collect_window(window: *mut c_void, lparam: isize) -> i32 {
        let search = unsafe { &mut *(lparam as *mut WindowSearch) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid != search.pid || unsafe { GetWindowTextLengthW(window) } <= 0 {
            return 1;
        }
        if unsafe { IsWindowVisible(window) } != 0 {
            if search.visible.is_none() {
                search.visible = Some(window);
            }
        } else if search.hidden.is_none() {
            search.hidden = Some(window);
        }
        1
    }

    /// 取某个进程的窗口：优先可见的，其次隐藏的（缩到托盘就是隐藏）。
    unsafe fn main_window_of(pid: u32) -> Option<*mut c_void> {
        let mut search = WindowSearch {
            pid,
            visible: None,
            hidden: None,
        };
        unsafe { EnumWindows(collect_window, &mut search as *mut WindowSearch as isize) };
        search.visible.or(search.hidden)
    }

    /// 把窗口显示出来并置顶。
    ///
    /// 「缩到托盘」是 `hide()`，窗口既不可见也不是最小化，所以必须显式 `SW_SHOW`
    /// 才能让它回来（`SW_RESTORE` 对隐藏窗口不生效）。
    unsafe fn activate(window: *mut c_void) {
        let command = if unsafe { IsIconic(window) } != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        unsafe { ShowWindow(window, command) };
        unsafe { SetForegroundWindow(window) };
    }

    /// 找回另一个 DShell 实例的主窗口并置顶；在 `timeout` 内没找到返回 `false`。
    ///
    /// 按**可执行文件路径**匹配，不按窗口标题：标题会随页面（WebView2 会把
    /// `document.title` 同步到窗口标题）和产品文案变化，而 exe 路径是稳的。
    /// 也不按窗口类名 —— 那是 tao 的实现细节。
    pub fn focus_existing_instance(timeout: Duration) -> bool {
        let Ok(current) = std::env::current_exe() else {
            return false;
        };
        let self_exe = current.to_string_lossy().to_string();
        let self_pid = unsafe { GetCurrentProcessId() };
        let deadline = Instant::now() + timeout;

        loop {
            let mut search = PidSearch {
                self_pid,
                self_exe: self_exe.clone(),
                found: None,
            };
            unsafe { EnumWindows(find_pid, &mut search as *mut PidSearch as isize) };
            if let Some(pid) = search.found {
                if let Some(window) = unsafe { main_window_of(pid) } {
                    unsafe { activate(window) };
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(FOCUS_RETRY);
        }
    }

    /// 等一个进程退出；已退出或打不开（当作已退出）都返回 `true`。
    ///
    /// 为什么不能只靠 `taskkill` 的返回码：那只能证明"信号发出去了"。
    /// 重起 dsh 前必须确认旧后端真的没了，否则会撞上它持有的会话锁（roadmap #1）。
    pub fn wait_process_exit(pid: u32, timeout: Duration) -> bool {
        if pid == 0 {
            return true;
        }
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return true;
        }
        let millis = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        let waited = unsafe { WaitForSingleObject(handle, millis) };
        unsafe { CloseHandle(handle) };
        waited == WAIT_OBJECT_0
    }

    /// 某个目录所在卷的可用空间（给用户看的 `FreeBytesAvailable`）。
    ///
    /// 为什么要问这个：换 dsh 版本要一次性重装 400+ 个包，写的是 npm 缓存所在的盘。
    /// 2026-09-21 实测踩过一次 —— 系统盘（256G SSD）只剩 12G / 已用 95%、而 npm 缓存
    /// 本身 15G，那次降级的写入高峰把整台机器拖死了（Kernel-Power 41 异常关机）。
    /// 面板据此在切换前给一句警告：接近写满的 SSD 上，写入突发可能拖住整个系统。
    pub fn free_space_bytes(dir: &str) -> Option<u64> {
        let path = wide(dir);
        let mut avail: u64 = 0;
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(path.as_ptr(), &mut avail, &mut total, &mut free)
        };
        (ok != 0).then_some(avail)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::time::Duration;

    use super::Instance;

    /// 非 Windows 平台没有实现：永远当作第一个实例（与改动前行为一致）。
    pub fn acquire_single_instance() -> Instance {
        Instance::Primary
    }

    /// 非 Windows 平台没有实现。
    pub fn focus_existing_instance(_timeout: Duration) -> bool {
        false
    }

    /// 非 Windows 平台没有实现：当作已退出。
    pub fn wait_process_exit(_pid: u32, _timeout: Duration) -> bool {
        true
    }

    /// 非 Windows 平台没有实现。
    pub fn free_space_bytes(_dir: &str) -> Option<u64> {
        None
    }
}

pub use imp::{
    acquire_single_instance, focus_existing_instance, free_space_bytes, wait_process_exit,
};
