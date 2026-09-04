use std::ptr::NonNull;

use ffmpeg_next as ffmpeg;
use tracing::{
    debug,
    trace,
    warn,
};

use super::{
    HWDeviceError,
    VideoFrame,
};

unsafe fn create_ffmpeg_hwctx(
    ty: ffmpeg::ffi::AVHWDeviceType
) -> Result<*mut ffmpeg::ffi::AVBufferRef, HWDeviceError> {
    unsafe {
        #[allow(clippy::wildcard_imports)]
        use ffmpeg::ffi::*;

        let mut ctx = std::ptr::null_mut();
        let err =
            av_hwdevice_ctx_create(&raw mut ctx, ty, std::ptr::null(), std::ptr::null_mut(), 0);
        if err != 0 {
            return Err(HWDeviceError::FFmpeg(ffmpeg::Error::from(err)));
        }

        Ok(ctx)
    }
}

pub struct FrameIterator {
    input:        ffmpeg::format::context::Input,
    decoder:      ffmpeg::decoder::Video,
    stream_index: usize,
}

impl FrameIterator {
    const HW_DECODERS: [ffmpeg::ffi::AVHWDeviceType; 7] = {
        #[allow(clippy::enum_glob_use)]
        use ffmpeg::ffi::AVHWDeviceType::*;
        [
            AV_HWDEVICE_TYPE_VULKAN,
            AV_HWDEVICE_TYPE_D3D12VA,
            AV_HWDEVICE_TYPE_D3D11VA,
            AV_HWDEVICE_TYPE_VAAPI,
            AV_HWDEVICE_TYPE_CUDA,
            AV_HWDEVICE_TYPE_AMF,
            AV_HWDEVICE_TYPE_QSV,
        ]
    };

    extern "C" fn get_format(
        ctx: *mut ffmpeg::ffi::AVCodecContext,
        pix_fmts: *const ffmpeg::ffi::AVPixelFormat,
    ) -> ffmpeg::ffi::AVPixelFormat {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            let ctx = ctx.as_mut_unchecked();

            if ctx.sw_pix_fmt == AVPixelFormat::AV_PIX_FMT_NONE {
                return AVPixelFormat::AV_PIX_FMT_NONE;
            }

            if let Some(hwdevice_ref) = ctx.hw_device_ctx.as_mut() {
                let device_ctx = hwdevice_ref
                    .data
                    .try_cast_aligned::<AVHWDeviceContext>()
                    .expect("FFmpeg didn't align hw_device_ctx");
                let device_ctx = device_ctx.as_mut_unchecked();

                let ty = device_ctx.type_;
                let mut i = 0;
                loop {
                    let config = avcodec_get_hw_config(ctx.codec, i);
                    if config.is_null() {
                        break;
                    }
                    i += 1;

                    let config = config.as_ref_unchecked();
                    if config.methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
                        && config.device_type == ty
                    {
                        let pixfmt = config.pix_fmt;

                        let mut p = pix_fmts;
                        while p.read() != AVPixelFormat::AV_PIX_FMT_NONE {
                            if p.read() == pixfmt {
                                let mut hwframes_ref = std::ptr::null_mut();
                                let mut err = avcodec_get_hw_frames_parameters(
                                    std::ptr::from_mut(ctx),
                                    std::ptr::from_mut(hwdevice_ref),
                                    pixfmt,
                                    &raw mut hwframes_ref,
                                );
                                if err == 0 {
                                    err = av_hwframe_ctx_init(hwframes_ref);
                                }

                                if err == 0 {
                                    ctx.hw_frames_ctx = hwframes_ref;

                                    trace!(?pixfmt, "Selected pixel format for HW Decoder");
                                    return pixfmt;
                                }

                                if !hwframes_ref.is_null() {
                                    av_buffer_unref(&raw mut hwframes_ref);
                                }

                                match ffmpeg::Error::from(err) {
                                    ffmpeg::Error::Other { errno }
                                        if errno == libc::ENOENT || errno == libc::EINVAL =>
                                    {
                                        trace!(
                                            ?pixfmt,
                                            "HW Decoding not supported; falling back to SW decoding"
                                        );
                                    },
                                    error => {
                                        warn!(
                                            ?error,
                                            "Encountered Error attempting to construct HWFrames Context; falling back to SW decoding"
                                        );
                                    },
                                }
                            }
                            p = p.add(1);
                        }
                    }
                }
            }

            let pixfmt = pix_fmts.read();
            if pixfmt != ctx.sw_pix_fmt {
                warn!("Non-Accelerated Pixel Format doesn't match first-pick format");
            }
            let pixfmt = ctx.sw_pix_fmt;
            trace!(?pixfmt, "Selected pixel format for SW Decoder");
            pixfmt
        }
    }

    pub fn new(
        mut input: ffmpeg::format::context::Input,
        hwdevice_ref: Option<&NonNull<ffmpeg::ffi::AVBufferRef>>,
    ) -> Result<Self, ffmpeg::Error> {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            let format_ctx = input.as_mut_ptr();
            let mut codec = std::ptr::null();
            let index = av_find_best_stream(
                format_ctx,
                AVMediaType::AVMEDIA_TYPE_VIDEO,
                -1,
                -1,
                &raw mut codec,
                0,
            );
            let stream = if index >= 0 {
                ffmpeg::Stream::wrap(&input, index.cast_unsigned() as usize)
            } else {
                return Err(ffmpeg::Error::StreamNotFound);
            };

            let mut ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;

            let mut i = 0;
            let mut supported_devices = Vec::new();
            loop {
                let config = avcodec_get_hw_config(codec, i);
                if config.is_null() {
                    // No more HW Configs exist; exit loop
                    break;
                }

                let config = config.read();
                if config.methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0 {
                    let ty = config.device_type;
                    let pixfmt = config.pix_fmt;
                    trace!(?ty, ?pixfmt, "Found HW Config");
                    supported_devices.push(ty);
                }

                i += 1;
            }

            // Initialize HW device
            for ty in Self::HW_DECODERS {
                if !supported_devices.contains(&ty) {
                    continue;
                }
                let ctx = ctx.as_mut_ptr().as_mut_unchecked();

                if let Some(hwdevice_ref) = hwdevice_ref {
                    let hwdevice_ctx = hwdevice_ref
                        .read()
                        .data
                        .try_cast_aligned::<AVHWDeviceContext>()
                        .expect("FFmpeg didn't align HWDevice Context");

                    if hwdevice_ctx.read().type_ == ty {
                        ctx.hw_device_ctx = av_buffer_ref(hwdevice_ref.as_ptr().cast_const());
                        break;
                    }

                    // Attempt to derive the target type
                    let err = av_hwdevice_ctx_create_derived(
                        &raw mut ctx.hw_device_ctx,
                        ty,
                        hwdevice_ref.as_ptr(),
                        0,
                    );
                    if err == 0 {
                        break;
                    }

                    let error = ffmpeg::Error::from(err);
                    debug!(?error, "Error while deriving hwdevice of type {:?}", ty);
                }

                match create_ffmpeg_hwctx(ty) {
                    Ok(hwdevice_ctx) => {
                        ctx.hw_device_ctx = hwdevice_ctx;
                        break;
                    },
                    Err(error) => {
                        warn!(?error, "Error while initializing hwdevice of type {:?}", ty);
                    },
                }
            }
            // Use HW device while decoding (set pixfmt)
            ctx.as_mut_ptr().as_mut_unchecked().get_format = Some(Self::get_format);

            let decoder = ctx.decoder().video()?;
            Ok(Self {
                stream_index: stream.index(),
                decoder,
                input,
            })
        }
    }

    pub fn format(&self) -> ffmpeg::format::Pixel {
        unsafe {
            let decoder = self.decoder.as_ptr().as_ref_unchecked();
            ffmpeg::format::Pixel::from(decoder.sw_pix_fmt)
        }
    }
}

impl Iterator for FrameIterator {
    type Item = Result<VideoFrame, ffmpeg::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut decoded_frame = VideoFrame::empty();

        loop {
            // See if decoder has a frame ready
            match self.decoder.receive_frame(&mut decoded_frame) {
                Ok(()) => return Some(Ok(decoded_frame)),
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::ffi::EAGAIN => {
                    // Need to feed more packets to the decoder
                },
                Err(ffmpeg::Error::Eof) => return None,
                Err(e) => return Some(Err(e)),
            }

            let mut packets = self.input.packets();
            if let Some((stream, packet)) = packets.next() {
                if stream.index() != self.stream_index {
                    continue;
                }

                match self.decoder.send_packet(&packet) {
                    Ok(()) => {},
                    Err(ffmpeg::Error::Eof) => {
                        return None;
                    },
                    Err(e) => {
                        return Some(Err(e));
                    },
                }
            // Sending EOF (NULL-packet) enters 'Draining mode', which will
            // never return EAGAIN on decoder->recv_frame;
            } else if let Err(e) = self.decoder.send_eof() {
                return Some(Err(e));
            }
        }
    }
}
