#![feature(pointer_try_cast_aligned)]

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

    // Create wgpu

    // TODO: Support OpenGL & D3D11
    let (instance, adapter, device, queue) = unsafe {
        let hal_instance = wgpu_hal::vulkan::Instance::init_with_callback(
            &wgpu_hal::InstanceDescriptor {
                name:                     "egui_vulkan",
                flags:                    wgpu::InstanceFlags::empty(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                backend_options:          wgpu::BackendOptions::default(),
                telemetry:                None,
                display:                  None,
            },
            Some(Box::new(|creation_args| {
                let mut count = 0;
                let extensions = av_vk_get_optional_instance_extensions(&raw mut count);
                let extensions = std::slice::from_raw_parts(extensions, count as usize);

                let Ok(binding) = creation_args
                    .entry
                    .enumerate_instance_extension_properties(None)
                else {
                    return;
                };
                let supported_extensions = binding
                    .iter()
                    .filter_map(|prop| prop.extension_name_as_c_str().ok())
                    .collect::<Vec<_>>();

                // Push (unique) ffmpeg extensions
                for bytes in extensions {
                    let cstr = CStr::from_ptr(*bytes);
                    if !creation_args.extensions.contains(&cstr)
                        && supported_extensions.contains(&cstr)
                    {
                        creation_args.extensions.push(cstr);
                    }
                }

                // List enabled extensions
                for extension in creation_args.extensions {
                    trace!(target: "lowpass_analysis::vulkan_init", ?extension, "Using Instance Extension");
                }
            })),
        )?;

        let selected_exposed = {
            let exposed_adapters = hal_instance.enumerate_adapters(None);

            let mut idx = 0;
            for (i, adapter) in exposed_adapters.iter().enumerate() {
                if adapter.info.device_type == wgpu::DeviceType::DiscreteGpu {
                    idx = i;
                }
            }
            exposed_adapters.into_iter().nth(idx)
        }
        .expect("No valid adapter available");
        let hal_adapter = &selected_exposed.adapter;

        let hal_device = hal_adapter.open_with_callback(
            selected_exposed.features,
            &selected_exposed.capabilities.limits,
            &wgpu::MemoryHints::default(),
            Some(Box::new(|creation_args| {
                let mut count = 0;
                let extensions = av_vk_get_optional_device_extensions(&raw mut count);
                let extensions = std::slice::from_raw_parts(extensions, count as usize);

                // Push (unique) ffmpeg extensions
                for bytes in extensions {
                    let cstr = CStr::from_ptr(*bytes);
                    if !creation_args.extensions.contains(&cstr)
                        && hal_adapter
                            .physical_device_capabilities()
                            .supports_extension(cstr)
                    {
                        creation_args.extensions.push(cstr);
                    }
                }

                // List enabled extensions
                for extension in creation_args.extensions {
                    trace!(
                        target: "lowpass_analysis::vulkan_init",
                        ?extension,
                        "Using Instance Extension"
                    );
                }

                // Can't see any obvious way to set the features; should be fine
            })),
        )?;

        let instance = wgpu::Instance::from_hal::<wgpu_hal::api::Vulkan>(hal_instance);
        let adapter = instance.create_adapter_from_hal(selected_exposed);
        let (device, queue) =
            adapter.create_device_from_hal(hal_device, &wgpu::DeviceDescriptor {
                label: Some("egui vulkan"),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 8192,
                    ..wgpu::Limits::defaults()
                },
                ..Default::default()
            })?;
        (instance, adapter, device, queue)
    };

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::Existing(egui_wgpu::WgpuSetupExisting {
                instance,
                adapter,
                device,
                queue,
            }),
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

    extern "C" fn get_format(
        ctx: *mut ffmpeg::ffi::AVCodecContext,
        pix_fmts: *const ffmpeg::ffi::AVPixelFormat,
    ) -> ffmpeg::ffi::AVPixelFormat {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            let ctx = ctx.as_mut_unchecked();

            if let Some(hw_device_ctx) = NonNull::new(ctx.hw_device_ctx) {
                let device_ctx = hw_device_ctx.read().buffer.cast::<AVHWDeviceContext>();
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
            trace!(?pixfmt, "Selected pixel format for HW Decoder");
            pixfmt
        }
    }

    extern "C" fn free_device_ctx(ctx: *mut ffmpeg::ffi::AVHWDeviceContext) {
        unsafe {
            if !ctx.read().hwctx.is_null() {
                ffmpeg::ffi::av_free(ctx.read().hwctx);
            }
            trace!("Free'd HW device ctx");
        }
    }

    fn create_vulkan_hwdevice(
        render_state: &egui_wgpu::RenderState
    ) -> Result<*mut ffmpeg::ffi::AVBufferRef, i32> {
        unsafe {
            #[allow(clippy::wildcard_imports)]
            use ffmpeg::ffi::*;

            let mut hw_device_ctx = av_hwdevice_ctx_alloc(AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN);
            if hw_device_ctx.is_null() {
                return Err(ffmpeg::Error::DecoderNotFound.into());
            }
            let raw_ctx = hw_device_ctx
                .read()
                .data
                .try_cast_aligned::<AVHWDeviceContext>()
                .expect("AVHWDeviceContext was not aligned");
            let ctx_ref = raw_ctx.as_mut_unchecked();

            let hal_instance = render_state
                .instance
                .as_hal::<wgpu_hal::api::Vulkan>()
                .expect("Unable to get Vulkan hal from render_state.instance");
            let hal_adapter = render_state
                .adapter
                .as_hal::<wgpu_hal::api::Vulkan>()
                .expect("Unable to get Vulkan hal from render_state.adapter");
            let hal_device = render_state
                .device
                .as_hal::<wgpu_hal::api::Vulkan>()
                .expect("Unable to get Vulkan hal from render_state.adapter");

            let vulkan_proc_addr = hal_instance
                .shared_instance()
                .entry()
                .static_fn()
                .get_instance_proc_addr;

            let mut device_features = ash::vk::PhysicalDeviceFeatures2::default();
            hal_device
                .shared_instance()
                .raw_instance()
                .get_physical_device_features2(
                    hal_device.raw_physical_device(),
                    &mut device_features,
                );

            let inst_extensions = hal_instance
                .shared_instance()
                .extensions()
                .iter()
                .map(|str| str.as_ptr())
                .collect::<Vec<_>>();
            let device_extensions = hal_device
                .enabled_device_extensions()
                .iter()
                .map(|str| str.as_ptr())
                .collect::<Vec<_>>();

            let mut qf = [AVVulkanDeviceQueueFamily {
                idx:        0,
                num:        0,
                flags:      0,
                video_caps: 0,
            }; 64]; // magic number moment

            let qf_len = hal_instance
                .shared_instance()
                .raw_instance()
                .get_physical_device_queue_family_properties2_len(hal_device.raw_physical_device());
            let mut video_props = vec![ash::vk::QueueFamilyVideoPropertiesKHR::default(); qf_len];
            let mut qf_props = video_props
                .iter_mut()
                .map(|vp| ash::vk::QueueFamilyProperties2::default().push_next(vp))
                .collect::<Vec<_>>();
            hal_instance
                .shared_instance()
                .raw_instance()
                .get_physical_device_queue_family_properties2(
                    hal_device.raw_physical_device(),
                    &mut qf_props,
                );

            let mut count = 0;

            for (idx, props) in qf_props.iter().enumerate() {
                qf[count].idx = idx as i32;
                qf[count].num = props.queue_family_properties.queue_count.cast_signed();
                qf[count].flags = props.queue_family_properties.queue_flags.as_raw();
                qf[count].video_caps = 0;

                let p_next = props
                    .p_next
                    .cast::<ash::vk::QueueFamilyVideoPropertiesKHR>();
                if p_next.is_null() {
                    continue;
                }

                qf[count].video_caps = p_next.read().video_codec_operations.as_raw().cast_signed();

                count += 1;
            }

            let device_qf_idx = hal_device.queue_family_index() as usize;
            if device_qf_idx < count {
                qf.copy_within(..count, 1);
                qf[0] = qf[device_qf_idx + 1];
                count += 1;
            }

            let hwctx = AVVulkanDeviceContext {
                alloc: std::ptr::null_mut(),
                #[allow(clippy::missing_transmute_annotations)] // I'm NOT annotating that
                get_proc_addr: None,
                inst: hal_instance
                    .shared_instance()
                    .raw_instance()
                    .handle()
                    .as_raw() as *mut _,
                phys_dev: hal_device.raw_physical_device().as_raw() as *mut _,
                act_dev: hal_device.raw_device().handle().as_raw() as *mut _,
                device_features: VkPhysicalDeviceFeatures2 {
                    sType:    device_features.s_type.as_raw(),
                    pNext:    device_features.p_next, // Hopefully this works
                    features: std::mem::transmute::<
                        ash::vk::PhysicalDeviceFeatures,
                        [u32; 55],
                    >(device_features.features),
                },
                enabled_inst_extensions: inst_extensions.as_ptr().cast(),
                nb_enabled_inst_extensions: i32::try_from(inst_extensions.len()).expect("More than i32::MAX instance extensions"),
                enabled_dev_extensions: device_extensions.as_ptr().cast(),
                nb_enabled_dev_extensions: i32::try_from(device_extensions.len()).expect("More than i32::MAX device extensions"),
                lock_queue: None,
                unlock_queue: None,
                qf,
                nb_qf: count as i32,
                queue_flags: 0,
            };
            ctx_ref.hwctx = av_malloc(size_of::<AVVulkanDeviceContext>());
            ctx_ref.hwctx.cast::<AVVulkanDeviceContext>().write(hwctx);
            ctx_ref.free = Some(Self::free_device_ctx);

            let err = av_hwdevice_ctx_init(hw_device_ctx);
            if err != 0 {
                av_buffer_unref(&raw mut hw_device_ctx);
                Err(err)
            } else {
                Ok(hw_device_ctx)
            }
        }
    }

    fn new(
        mut input: ffmpeg::format::context::Input,
        render_state: &egui_wgpu::RenderState,
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

                // If hwdevice is same as wgpu hal, use that
                let err = match ty {
                    AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN => {
                        let res = Self::create_vulkan_hwdevice(render_state);
                        match res {
                            Ok(hw_device_ctx) => {
                                ctx.hw_device_ctx = hw_device_ctx;
                                0
                            },
                            Err(err) => err,
                        }
                    },
                    _ => av_hwdevice_ctx_create(
                        &raw mut ctx.hw_device_ctx,
                        ty,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        0,
                    ),
                };

                if err != 0 {
                    let error = ffmpeg::Error::from(err);
                    warn!(?error, "Error while initializing hwdevice of type {:?}", ty);
                    if !ctx.hw_device_ctx.is_null() {
                        av_buffer_unref(&raw mut ctx.hw_device_ctx);
                    }
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
                Ok(_) => return Some(Ok(decoded_frame.clone())),
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

    current: Option<[egui::TextureHandle; 2]>,
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
            assert!(!vkframe.is_null(), "AVVkFrame was null");
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
    ) -> egui::TextureHandle {
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
        )
    }

    #[allow(clippy::missing_const_for_fn)]
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, InitError> {
        let render_state = cc.wgpu_render_state.as_ref().expect("Missing render state");

        // Load video streams
        // TODO: Allow this to be done via UI instead

        let clean_ctx = ffmpeg::format::input(Self::CLEAN_VIDEO_PATH)?;
        let mut clean_frames = FrameIterator::new(clean_ctx, render_state)?;
        for _ in 0..24 {
            _ = clean_frames.next();
        }

        let dirty_ctx = ffmpeg::format::input(Self::LOWPASSED_VIDEO_PATH)?;
        let mut dirty_frames = FrameIterator::new(dirty_ctx, render_state)?;

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
                for handle in frames {
                    ui.add(
                        egui::Image::from_texture(egui::load::SizedTexture::from_handle(handle))
                            .maintain_aspect_ratio(true)
                            .fit_to_exact_size(egui::vec2(width, size.y)),
                    );
                }
            });

            Some(())
        });
    }
}
