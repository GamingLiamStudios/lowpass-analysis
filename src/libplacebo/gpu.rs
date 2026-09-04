use std::{
    marker::PhantomData,
    mem::MaybeUninit,
    ptr::NonNull,
};

use ffmpeg_next::format::Pixel;

use super::ffi;

pub struct Texture<'a> {
    handle: ffi::pl_tex,
    gpu:    Gpu<'a>,
}

impl Drop for Texture<'_> {
    fn drop(&mut self) {
        unsafe {
            ffi::pl_tex_destroy(self.gpu.handle, &raw mut self.handle);
        }
    }
}

pub enum FrameSource<'a> {
    AVFrame {
        gpu:     Gpu<'a>,
        texture: Option<Texture<'a>>,
    },
}

pub struct Frame<'a> {
    inner:  ffi::pl_frame,
    source: FrameSource<'a>,
}

impl<'a> Frame<'a> {
    pub fn drop_retain(mut self) -> Option<Texture<'a>> {
        match &mut self.source {
            FrameSource::AVFrame { gpu: _, texture } => {
                texture.take() // Implicity calls Frame destructor
            },
        }
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        unsafe {
            match &self.source {
                FrameSource::AVFrame { gpu, texture: _ } => {
                    ffi::pl_unmap_avframe(gpu.handle, &raw mut self.inner);
                },
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Gpu<'a> {
    pub(super) handle:  ffi::pl_gpu,
    pub(super) _marker: PhantomData<&'a ()>,
}

impl Gpu<'_> {
    pub fn is_pixfmt_supported(
        &self,
        pixfmt: Pixel,
    ) -> bool {
        unsafe {
            ffi::pl_test_pixfmt(
                self.handle,
                std::convert::Into::<ffmpeg_next::ffi::AVPixelFormat>::into(pixfmt) as i32,
            )
        }
    }

    pub fn import_avframe(
        &self,
        avframe: &ffmpeg_next::Frame,
        backing_texture: Option<Texture>,
        map_dovi: bool,
    ) -> Option<Frame<'_>> {
        unsafe {
            let mut tex = backing_texture.map_or(std::ptr::null(), |tex| tex.handle);
            let params = ffi::pl_avframe_params {
                frame: avframe.as_ptr().cast(),
                tex: &raw mut tex,
                map_dovi,
            };

            let mut frame = MaybeUninit::zeroed();
            if ffi::pl_map_avframe_ex(self.handle, frame.as_mut_ptr(), &raw const params) {
                let texture = NonNull::new(tex.cast_mut()).map(|ptr| Texture {
                    handle: ptr.as_ptr().cast_const(),
                    gpu:    *self,
                });
                Some(Frame {
                    inner:  frame.assume_init(),
                    source: FrameSource::AVFrame {
                        gpu: *self,
                        texture,
                    },
                })
            } else {
                None
            }
        }
    }
}
