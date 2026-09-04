#![feature(pointer_try_cast_aligned)]

use std::{
    collections::HashMap,
    hash::Hash,
    ptr::NonNull,
    rc::Rc,
};

use egui::ViewportBuilder;
use ffmpeg::frame::Video as VideoFrame;
use ffmpeg_next::{
    self as ffmpeg,
    format::Pixel,
};
use tracing::{
    Level,
    debug,
    error,
    info,
    trace,
    warn,
};
use tracing_subscriber::{
    Layer,
    filter::Targets,
    layer::SubscriberExt,
};

use crate::{
    frame_iterator::FrameIterator,
    vulkan::VulkanBackend,
};

mod frame_iterator;
mod libplacebo;
mod vulkan;

#[derive(thiserror::Error, Debug)]
enum WgpuInitError {
    #[error("Failed to create Instance")]
    InstanceCreate(#[from] wgpu_hal::InstanceError),

    #[error("Instance version too low (ffmpeg requires >=1.3, got {0})")]
    VulkanVersion(String),

    #[error("Requested Physical Device ({0}) does not exist")]
    RequestedPhysicalDevice(String),

    #[error("Failed to open Device")]
    DeviceOpen(ash::vk::Result),

    #[error("Failed to open Device with HAL")]
    DeviceOpenHal(#[from] wgpu_hal::DeviceError),

    #[error("Failed to request Device")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),

    #[error("Failed to find Graphics Queue for Device")]
    DeviceGraphicsQueue,

    #[error("Physical Device does not support required features")]
    PhysicalDeviceUnsupported,
}

#[derive(Debug, thiserror::Error)]
enum HWDeviceError {
    #[error("Failed to allocate AVHWDeviceContext")]
    AllocationFailed,

    #[error("FFmpeg returned an error")]
    FFmpeg(#[from] ffmpeg::Error),

    #[error("Libplacebo failed to run {0}")]
    LibPlacebo(&'static str),
}

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

    let placebo_log = libplacebo::LogParams {
        level:    libplacebo::LogLevel::ALL,
        callback: |level, msg| {
            use libplacebo::LogLevel;
            match level {
                LogLevel::None => (),
                LogLevel::Fatal | LogLevel::Err => {
                    tracing::error!(target: "libplacebo", "{msg}");
                },
                LogLevel::Warn => tracing::warn!(target: "libplacebo", "{msg}"),
                LogLevel::Info => tracing::info!(target: "libplacebo", "{msg}"),
                LogLevel::Debug => tracing::debug!(target: "libplacebo", "{msg}"),
                LogLevel::Trace => tracing::trace!(target: "libplacebo", "{msg}"),
            }
        },
    }
    .build();

    // TODO: Support OGL and DX12
    let backend = VulkanBackend::new()?;

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::Existing(backend.egui_existing()),
            ..eframe::WgpuConfiguration::default()
        },
        viewport: ViewportBuilder::default()
            .with_app_id("org.glstudios.lowpass_analysis")
            .with_title("Lowpass Analysis Tool"),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "org.glstudios.lowpass_analysis",
        options,
        Box::new(|cc| {
            let app = App::new(cc, backend, placebo_log)?;
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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct FrameProps {
    width:  u32,
    height: u32,
    pixfmt: Pixel,
}

impl Hash for FrameProps {
    fn hash<H: std::hash::Hasher>(
        &self,
        state: &mut H,
    ) {
        state.write_u32(self.width);
        state.write_u32(self.height);
        state.write_u32(self.pixfmt as u32);
    }
}

struct App<'vulkan, 'placebo: 'vulkan> {
    clean:     FrameIterator,
    lowpassed: FrameIterator,

    backend:       Rc<VulkanBackend<'vulkan>>,
    placebo:       libplacebo::vulkan::GpuVk<'placebo>,
    texture_cache: HashMap<FrameProps, Vec<libplacebo::Texture<'placebo>>>,
}

impl<'vulkan, 'placebo> App<'vulkan, 'placebo> {
    const CLEAN_VIDEO_PATH: &'static str = "videos/clean.m2ts";
    const LOWPASSED_VIDEO_PATH: &'static str = "videos/lowpassed.m2ts";

    fn release_texture(
        &mut self,
        frame: &VideoFrame,
    ) -> Option<libplacebo::Texture<'placebo>> {
        let props = FrameProps {
            width:  frame.width(),
            height: frame.height(),
            pixfmt: frame.format(),
        };

        self.texture_cache
            .get_mut(&props)
            .map_or_else(|| None, std::vec::Vec::pop)
    }

    fn consume_texture(
        &mut self,
        frame: libplacebo::Frame<'placebo>,
    ) {
        let src_frame = frame
            .source_avframe()
            .expect("This shouldn't be anything but an AVFrame");

        let props = FrameProps {
            width:  src_frame.width(),
            height: src_frame.height(),
            pixfmt: src_frame.format(),
        };

        if let Some(texture) = frame.drop_retain() {
            texture.invalidate();
            self.texture_cache.entry(props).or_default().push(texture);
        }
    }

    fn process_pair(&mut self) {
        // To process pair of frames;
        // - Decode frames
        // - Copy over to libplacebo
        // - Reduce to luma
        // - Test many, many lowpasses & determine best-fit
        let gpu = self.placebo.as_gpu();

        let Some(next_clean) = self.clean.next() else {
            return; // End of stream
        };
        let clean_frame = match next_clean {
            Ok(frame) => frame,
            Err(error) => {
                error!(?error, "Encountered error while decoding frame");
                return;
            },
        };

        let Some(next_dirty) = self.lowpassed.next() else {
            return; // End of stream
        };
        let dirty_frame = match next_dirty {
            Ok(frame) => frame,
            Err(error) => {
                error!(?error, "Encountered error while decoding frame");
                return;
            },
        };

        let clean_tex = self.release_texture(&clean_frame);
        let Some(clean_frame) = gpu.import_avframe(&clean_frame, clean_tex, false) else {
            error!("Failed to transfer AVFrame to Libplacebo");
            return;
        };

        let dirty_tex = self.release_texture(&dirty_frame);
        let Some(dirty_frame) = gpu.import_avframe(&dirty_frame, dirty_tex, false) else {
            error!("Failed to transfer AVFrame to Libplacebo");
            return;
        };

        info!("Collected frames with libplacebo");

        // Return textures to cache
        self.consume_texture(clean_frame);
        self.consume_texture(dirty_frame);
    }

    #[allow(clippy::missing_const_for_fn)]
    fn new(
        cc: &eframe::CreationContext<'_>,
        backend: Rc<VulkanBackend<'vulkan>>,
        placebo_log: libplacebo::Log,
    ) -> Result<Self, InitError> {
        let render_state = cc.wgpu_render_state.as_ref().expect("Missing render state");

        // Initalize ffmpeg and libplacebo
        let hwdevice_ctx = match backend.create_ffmpeg_hwctx() {
            Ok(ctx) => NonNull::new(ctx),
            Err(error) => {
                warn!(?error, "Failed to create hwdevice_ctx with wgpu");
                None
            },
        };
        let libplacebo_ctx = match backend.create_libplacebo_ctx(placebo_log) {
            Ok(ctx) => ctx,
            Err(error) => {
                error!(?error, "Failed to create LibPlacebo context with wgpu");
                todo!(
                    "Creating LibPlacebo context outside of existing wgpu device is not yet implemented"
                ) // TODO
            },
        };

        // Load video streams
        // TODO: Allow this to be done via UI instead

        let clean_ctx = ffmpeg::format::input(Self::CLEAN_VIDEO_PATH)?;
        let mut clean_frames = FrameIterator::new(clean_ctx, hwdevice_ctx.as_ref())?;
        for _ in 0..24 {
            _ = clean_frames.next();
        }

        let dirty_ctx = ffmpeg::format::input(Self::LOWPASSED_VIDEO_PATH)?;
        let dirty_frames = FrameIterator::new(dirty_ctx, hwdevice_ctx.as_ref())?;

        Ok(Self {
            clean:         clean_frames,
            lowpassed:     dirty_frames,
            backend:       backend.clone(),
            placebo:       libplacebo_ctx,
            texture_cache: HashMap::new(),
        })
    }
}

impl eframe::App for App<'_, '_> {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
    ) {
        let _render_state = frame.wgpu_render_state().expect("Not using WGPU for egui");

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("So you want to delowpass");
            if ui.button("test").clicked() {
                self.process_pair();
            }
        });
    }
}
