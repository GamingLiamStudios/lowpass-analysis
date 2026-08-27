#![feature(pointer_try_cast_aligned, c_variadic)]

use core::panic;
use std::{
    collections::HashMap,
    ffi::CStr,
    os::unix::raw,
    path,
    ptr::NonNull,
    sync::Arc,
};

use ash::vk::Handle;
use egui::{
    Vec2,
    ViewportBuilder,
};
use ffmpeg::frame::Video as VideoFrame;
use ffmpeg_next::{
    self as ffmpeg,
    ffi::{
        av_vk_get_optional_device_extensions,
        av_vk_get_optional_instance_extensions,
    },
    format::Pixel,
    frame::Flags,
};
use tracing::{
    Level,
    debug,
    error,
    trace,
    warn,
};
use tracing_subscriber::{
    Layer,
    filter::Targets,
    layer::SubscriberExt,
};
use wgpu_hal::{
    Adapter,
    Instance,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdout = tracing_subscriber::fmt::Layer::default()
        .with_writer(std::io::stdout)
        .with_ansi(true);
    let registry = tracing_subscriber::registry().with(
        stdout.with_filter(
            Targets::default()
                .with_target("lowpass_analysis", Level::TRACE)
                .with_default(Level::INFO),
        ),
    );
    tracing::subscriber::set_global_default(registry)?;

    tracing::info!("Hello, world!");
    ffmpeg::init()?;

    //unsafe {
    //    ffmpeg::ffi::av_log_set_level(ffmpeg::ffi::AV_LOG_VERBOSE);
    //}

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: ViewportBuilder::default()
            .with_app_id("org.glstudios.lowpass_analysis")
            .with_title("Lowpass Analysis Tool"),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "org.glstudios.lowpass_analysis",
        options,
        Box::new(|cc| {
            let app = App::new(cc)?;
            Ok(Box::new(app))
        }),
    )?;

    Ok(())
}

#[derive(thiserror::Error, Debug)]
enum InitError {
    #[error("Failed to load video stream")]
    VideoStreamLoadError(#[from] ffmpeg::Error),
}

struct FrameIterator {
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
    const INITIAL_POOL_SIZE: i32 = 24;

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

            if let Some(hw_device_ctx) = NonNull::new(ctx.hw_device_ctx) {
                let device_ctx = hw_device_ctx
                    .read()
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
                                trace!(?pixfmt, "Selected pixel format for HW Decoder");

                                return pixfmt;
                            }
                            p = p.add(1);
                        }
                    }
                }
            }

            let pixfmt = pix_fmts.read();
            trace!(?pixfmt, "Selected pixel format for Decoder");
            pixfmt
        }
    }

    fn new(mut input: ffmpeg::format::context::Input) -> Result<Self, ffmpeg::Error> {
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
                ffmpeg::Stream::wrap(&input, index as usize)
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

                let err = av_hwdevice_ctx_create(
                    &raw mut ctx.hw_device_ctx,
                    ty,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                );
                if err != 0 {
                    let error = ffmpeg::Error::from(err);
                    warn!(?error, "Error while initializing hwdevice of type {:?}", ty);
                    continue;
                }

                break;
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
                Ok(()) => {
                    //unsafe {
                    //    let ctx = self.decoder.as_ptr().read();
                    //    trace!(hwaccel = ?ctx.hwaccel, hw_frames = ?ctx.hw_frames_ctx);
                    //}
                    return Some(Ok(decoded_frame));
                },
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
                    Err(ffmpeg::Error::Eof) => return None,
                    Err(e) => return Some(Err(e)),
                }
            // Sending EOF (NULL-packet) enters 'Draining mode', which will
            // never return EAGAIN on decoder->recv_frame;
            } else if let Err(e) = self.decoder.send_eof() {
                return Some(Err(e));
            }
        }
    }
}

struct App {
    clean:     FrameIterator,
    lowpassed: FrameIterator,

    current: Option<[(); 2]>,
}

impl App {
    const CLEAN_VIDEO_PATH: &'static str = "videos/clean.m2ts";
    const LOWPASSED_VIDEO_PATH: &'static str = "videos/lowpassed.m2ts";

    fn process_vulkan_frame(
        ctx: &egui::Context,
        render_state: &egui_wgpu::RenderState,
        name: &str,
        frame: &mut VideoFrame,
        pixfmt: &ffmpeg::format::Pixel,
    ) {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            let avframe = frame.as_mut_ptr();
            let avf = avframe.read();
            trace!(hwframe = ?avf.hw_frames_ctx);

            let pix_desc = pixfmt.descriptor().expect("Invalid Pixel Format");
            let pix_desc = pix_desc.as_ptr().read();

            let flags = pix_desc.flags;
            let is_rgb = flags & AV_PIX_FMT_FLAG_RGB as u64 != 0;
            let is_planar = flags & AV_PIX_FMT_FLAG_PLANAR as u64 != 0;
            let channels = pix_desc.nb_components;
            let depth = pix_desc.comp[0].depth;

            if is_rgb || !is_planar || channels != 3 {
                unimplemented!("HW Pixel Format isn't YUV");
            }

            let format = match depth {
                0..=8 => wgpu::TextureFormat::R8Unorm,
                9..=16 => wgpu::TextureFormat::R16Unorm,
                depth => {
                    warn!(?depth, "Unsupported HW pixel depth");
                    unimplemented!("Unsupported HW Pixel Format");
                },
            };
            trace!(texture = ?format, source = ?pixfmt, "Selected format");

            // Extract Luma (plane 0) of frame
            let vkframe = avframe.read().data[0]
                .try_cast_aligned::<AVVkFrame>()
                .expect("AVVkFrame wasn't aligned");
            if vkframe.is_null() {
                warn!("AVVkFrame was null");
                return;
            }
            let vkframe = vkframe.read();

            let luma_plane = std::mem::transmute::<u64, ash::vk::Image>(vkframe.img[0]);
            let width = frame.width();
            let height = frame.height();

            // Try cast egui backend to Vulkan Hal
            if let Some(vk_device) = render_state.device.as_hal::<wgpu_hal::api::Vulkan>() {
                // Directly copy from vulkan to vulkan
                let vk_device = &*vk_device;

                let vk_texture = vk_device.texture_from_raw(
                    luma_plane,
                    &wgpu_hal::TextureDescriptor {
                        label: Some("ffmpeg_vulkan_frame"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUses::RESOURCE,
                        memory_flags: wgpu_hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    },
                    None,
                    wgpu_hal::vulkan::TextureMemory::External,
                );
                render_state
                    .device
                    .create_texture_from_hal::<wgpu_hal::api::Vulkan>(
                        vk_texture,
                        &wgpu::TextureDescriptor {
                            label: Some("ffmpeg_vulkan_frame"),
                            size: wgpu::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        wgpu::TextureUses::RESOURCE,
                    );
            } else {
                // Use external memory to copy from vulkan to device
            }
        }
    }

    fn video_frame_to_egui(
        ctx: &egui::Context,
        render_state: &egui_wgpu::RenderState,
        name: &str,
        frame: &mut VideoFrame,
        pixfmt: &ffmpeg::format::Pixel,
    ) {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            let pix_desc = frame.format().descriptor().expect("Invalid Pixel Format");
            let pix_desc = pix_desc.as_ptr().read();

            if pix_desc.flags & AV_PIX_FMT_FLAG_HWACCEL as u64 != 0 {
                // Process as HW frame
                let texture = match frame.format() {
                    Pixel::VULKAN => {
                        Self::process_vulkan_frame(ctx, render_state, name, frame, pixfmt)
                    },
                    _ => unimplemented!("Unsupported HW frame type"),
                };
                return;
            } else {
                // Process as SW frame
                if pix_desc.flags & AV_PIX_FMT_FLAG_RGB as u64 != 0
                    || pix_desc.flags & AV_PIX_FMT_FLAG_PLANAR as u64 == 0
                    || pix_desc.comp[0].depth != 8
                {
                    todo!("Scale with ffmpeg to f32 Luma") // TODO
                }
            }
        }

        let width = frame.plane_width(0) as usize;
        let height = frame.plane_height(0) as usize;
        let mut buffer = vec![0u8; width * height];

        for h in 0..height {
            let src_offs = h * frame.stride(0);
            let dst_offs = h * width;
            buffer[dst_offs..dst_offs + width]
                .copy_from_slice(&frame.data(0)[src_offs..src_offs + width]);
        }

        let image = egui::ColorImage::from_gray([width, height], &buffer);
        ctx.load_texture(
            name,
            egui::ImageData::Color(image.into()),
            egui::TextureOptions::NEAREST,
        );
    }

    #[allow(clippy::missing_const_for_fn)]
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, InitError> {
        let render_state = cc.wgpu_render_state.as_ref().expect("Missing render state");

        // Load video streams
        // TODO: Allow this to be done via UI instead

        let clean_ctx = ffmpeg::format::input(Self::CLEAN_VIDEO_PATH)?;
        let mut clean_frames = FrameIterator::new(clean_ctx)?;
        for _ in 0..24 {
            _ = clean_frames.next();
        }

        let dirty_ctx = ffmpeg::format::input(Self::LOWPASSED_VIDEO_PATH)?;
        let mut dirty_frames = FrameIterator::new(dirty_ctx)?;

        // Grab first frame of each
        let frames = match (clean_frames.next(), dirty_frames.next()) {
            (Some(Ok(mut clean)), Some(Ok(mut dirty))) => {
                let clean = Self::video_frame_to_egui(
                    &cc.egui_ctx,
                    render_state,
                    "clean_frame",
                    &mut clean,
                    &clean_frames.format(),
                );
                let dirty = Self::video_frame_to_egui(
                    &cc.egui_ctx,
                    render_state,
                    "dirty_frame",
                    &mut dirty,
                    &dirty_frames.format(),
                );
                Some([clean, dirty])
            },
            (Some(Err(e)), _) | (_, Some(Err(e))) => {
                return Err(InitError::VideoStreamLoadError(e));
            },
            (None, _) | (_, None) => None,
        };

        Ok(Self {
            clean:     clean_frames,
            lowpassed: dirty_frames,
            current:   frames,
        })
    }
}

impl eframe::App for App {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
    ) {
        let render_state = frame.wgpu_render_state().expect("Not using WGPU for egui");

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("So you want to delowpass");
            if ui.button("how about a new frame old man").clicked() {
                let frames = match (self.clean.next(), self.lowpassed.next()) {
                    (Some(Ok(mut clean)), Some(Ok(mut dirty))) => {
                        let clean = Self::video_frame_to_egui(
                            ui.ctx(),
                            render_state,
                            "clean_frame",
                            &mut clean,
                            &self.clean.format(),
                        );
                        let dirty = Self::video_frame_to_egui(
                            ui.ctx(),
                            render_state,
                            "dirty_frame",
                            &mut dirty,
                            &self.lowpassed.format(),
                        );
                        Some([clean, dirty])
                    },
                    (Some(Err(e)), _) | (_, Some(Err(e))) => {
                        // FIXME: Throw error
                        return None;
                    },
                    (None, _) | (_, None) => None,
                };
                self.current = frames;
            }

            // Display frames
            let size = ui.available_size_before_wrap();
            ui.horizontal(|ui| {
                let Some(frames) = self.current.as_ref() else {
                    return;
                };

                let width = size.x / (frames.len() as f32);
                //for handle in frames {
                //    ui.add(
                //        egui::Image::from_texture(egui::load::SizedTexture::from_handle(handle))
                //            .maintain_aspect_ratio(true)
                //            .fit_to_exact_size(egui::vec2(width, size.y)),
                //    );
                //}
            });

            Some(())
        });
    }
}
