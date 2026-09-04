use std::{
    ffi::CStr,
    rc::Rc,
};

use super::ffi;
use crate::libplacebo::ffi::PL_API_VER;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LogLevel {
    #[default]
    None = 0,
    Fatal = 1,
    Err = 2,
    Warn = 3,
    Info = 4,
    Debug = 5,
    Trace = 6,
}

impl LogLevel {
    pub const ALL: Self = Self::Trace;

    pub const fn from_raw(level: u32) -> Option<Self> {
        match level {
            ffi::PL_LOG_NONE => Some(Self::None),
            ffi::PL_LOG_FATAL => Some(Self::Fatal),
            ffi::PL_LOG_ERR => Some(Self::Err),
            ffi::PL_LOG_WARN => Some(Self::Warn),
            ffi::PL_LOG_INFO => Some(Self::Info),
            ffi::PL_LOG_DEBUG => Some(Self::Debug),
            ffi::PL_LOG_TRACE => Some(Self::Trace),
            _ => None,
        }
    }

    pub const fn as_raw(self) -> u32 {
        match self {
            Self::None => ffi::PL_LOG_NONE,
            Self::Fatal => ffi::PL_LOG_FATAL,
            Self::Err => ffi::PL_LOG_ERR,
            Self::Warn => ffi::PL_LOG_WARN,
            Self::Info => ffi::PL_LOG_INFO,
            Self::Debug => ffi::PL_LOG_DEBUG,
            Self::Trace => ffi::PL_LOG_TRACE,
        }
    }
}

#[derive(Debug)]
pub struct LogParams {
    pub level:    LogLevel,
    pub callback: fn(LogLevel, &str),
}

extern "C" fn log_callback(
    ctx: *mut std::ffi::c_void,
    level: u32,
    msg: *const i8,
) {
    unsafe {
        let level = LogLevel::from_raw(level).expect("libplacebo returned invalid LogLevel");
        let msg = CStr::from_ptr(msg).to_string_lossy();

        let callback = std::mem::transmute::<*mut _, fn(LogLevel, &str)>(ctx);
        (callback)(level, &msg);
    }
}

impl Default for LogParams {
    fn default() -> Self {
        Self {
            level:    LogLevel::None,
            callback: |_, _| {},
        }
    }
}

impl LogParams {
    #[must_use]
    pub fn build(self) -> Log {
        unsafe {
            let params = ffi::pl_log_params {
                log_level: self.level.as_raw(),
                log_priv:  self.callback as *mut std::ffi::c_void,
                log_cb:    Some(log_callback),
            };
            Log(Rc::new(
                ffi::pl_log_create_360(PL_API_VER, &raw const params).into(),
            ))
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone)]
struct LogInner(ffi::pl_log);

impl Drop for LogInner {
    fn drop(&mut self) {
        unsafe {
            ffi::pl_log_destroy(&raw mut self.0);
        }
    }
}

impl From<ffi::pl_log> for LogInner {
    fn from(value: ffi::pl_log) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone)]
pub struct Log(Rc<LogInner>);

impl Default for Log {
    fn default() -> Self {
        unsafe { Self::from_raw(std::ptr::null()) }
    }
}

impl Log {
    pub fn handle(&self) -> ffi::pl_log {
        self.0.0
    }

    pub unsafe fn from_raw(handle: ffi::pl_log) -> Self {
        Self(Rc::new(handle.into()))
    }
}
