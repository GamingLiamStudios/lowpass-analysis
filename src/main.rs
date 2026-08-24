use std::path;

use egui::{
    Vec2,
    ViewportBuilder,
};
use ffmpeg::frame::Video as VideoFrame;
use ffmpeg_next as ffmpeg;
use tracing::Level;
use tracing_subscriber::{
    Layer,
    filter::Targets,
    layer::SubscriberExt,
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
    flushing:     bool,
}

impl FrameIterator {
    fn new(input: ffmpeg::format::context::Input) -> Result<Self, ffmpeg::Error> {
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().video()?;

        Ok(Self {
            stream_index: stream.index(),
            decoder,
            input,
            flushing: false,
        })
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

            if self.flushing {
                return None;
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
            } else {
                self.flushing = true;
                if let Err(e) = self.decoder.send_eof() {
                    return Some(Err(e));
                }
            }
        }
    }
}

struct App {
    clean_frames:     FrameIterator,
    lowpassed_frames: FrameIterator,

    current_frames: Option<[egui::TextureHandle; 2]>,
}

impl App {
    const CLEAN_VIDEO_PATH: &'static str = "videos/clean.m2ts";
    const LOWPASSED_VIDEO_PATH: &'static str = "videos/lowpassed.m2ts";

    fn video_frame_to_egui(
        ctx: &egui::Context,
        name: &str,
        frame: &VideoFrame,
    ) -> egui::TextureHandle {
        // TODO: Scale with ffmpeg to f32 precision
        assert!(
            frame.format() == ffmpeg::format::Pixel::YUV420P,
            "fixme please"
        );

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
            (Some(Ok(clean)), Some(Ok(dirty))) => {
                let clean = Self::video_frame_to_egui(&cc.egui_ctx, "clean_frame", &clean);
                let dirty = Self::video_frame_to_egui(&cc.egui_ctx, "dirty_frame", &dirty);
                Some([clean, dirty])
            },
            (Some(Err(e)), _) | (_, Some(Err(e))) => {
                return Err(InitError::VideoStreamLoadError(e));
            },
            (None, _) | (_, None) => None,
        };

        Ok(Self {
            clean_frames,
            lowpassed_frames: dirty_frames,
            current_frames: frames,
        })
    }
}

impl eframe::App for App {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
    ) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("So you want to delowpass");
            if ui.button("how about a new frame old man").clicked() {
                let frames = match (self.clean_frames.next(), self.lowpassed_frames.next()) {
                    (Some(Ok(clean)), Some(Ok(dirty))) => {
                        let clean = Self::video_frame_to_egui(ui.ctx(), "clean_frame", &clean);
                        let dirty = Self::video_frame_to_egui(ui.ctx(), "dirty_frame", &dirty);
                        Some([clean, dirty])
                    },
                    (Some(Err(e)), _) | (_, Some(Err(e))) => {
                        // FIXME: Throw error
                        return None;
                    },
                    (None, _) | (_, None) => None,
                };
                self.current_frames = frames;
            }

            // Display frames
            let size = ui.available_size_before_wrap();
            ui.horizontal(|ui| {
                let Some(frames) = self.current_frames.as_ref() else {
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
