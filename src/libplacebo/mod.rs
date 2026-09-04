pub mod ffi {
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(unused_imports)]

    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub mod log;
pub use log::{
    Log,
    LogLevel,
    LogParams,
};

pub mod gpu;
pub use gpu::{
    Frame,
    Gpu,
    Texture,
};
pub mod vulkan;
