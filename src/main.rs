use eframe::egui;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::sync::{Arc, Mutex};
use std::fs;
use serde::{Deserialize, Serialize};
use directories::{ProjectDirs, UserDirs};

fn main() -> eframe::Result<()> {
    std::env::set_var("GST_V4L2_USE_LIBV4L2", "1");
    gst::init().expect("Failed to initialize GStreamer. Ensure GStreamer core libraries are installed.");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
        .with_inner_size([800.0, 600.0])
        .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "gst-cam-rs",
        options,
        Box::new(|cc| Ok(Box::new(WebcamApp::new(cc.egui_ctx.clone())))),
    )
}

struct CameraInfo {
    display_name: String,
    device: gst::Device,
}

#[derive(Clone, PartialEq, Debug)]
struct FormatOption {
    width: i32,
    height: i32,
    fps: i32,
    is_mjpeg: bool,
}

impl FormatOption {
    fn to_caps(&self) -> gst::Caps {
        let name = if self.is_mjpeg { "image/jpeg" } else { "video/x-raw" };
        gst::Caps::builder(name)
        .field("width", self.width)
        .field("height", self.height)
        .field("framerate", gst::Fraction::new(self.fps, 1))
        .build()
    }

    fn display_name(&self) -> String {
        let format = if self.is_mjpeg { "MJPEG" } else { "RAW" };
        format!("{}x{} @ {}fps ({})", self.width, self.height, self.fps, format)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(default)]
struct AppSettings {
    save_directory: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        let save_dir = UserDirs::new()
        .and_then(|dirs| dirs.video_dir().map(|p| p.join("gst-cam-rs").to_string_lossy().into_owned()))
        .unwrap_or_else(|| "/tmp/gst-cam-rs".to_string());

        Self {
            save_directory: save_dir,
        }
    }
}

struct WebcamApp {
    cameras: Vec<CameraInfo>,
    selected_camera_idx: usize,
    available_formats: Vec<FormatOption>,
    selected_format_idx: usize,
    is_recording: bool,
    texture: Option<egui::TextureHandle>,
    latest_frame: Arc<Mutex<Option<egui::ColorImage>>>,
    flip_horizontal: bool,
    flip_vertical: bool,
    rotation: f32,
    pipeline: Option<gst::Pipeline>,
    recording_pipeline: Option<gst::Pipeline>,
    record_src: Arc<Mutex<Option<gst_app::AppSrc>>>,
    capture_request: Arc<Mutex<Option<String>>>,
    settings: AppSettings,
    show_settings: bool,
}

impl WebcamApp {
    fn new(ctx: egui::Context) -> Self {
        let config_path = Self::get_config_path();
        let settings = match fs::read_to_string(&config_path) {
            Ok(json_str) => serde_json::from_str(&json_str).unwrap_or_default(),
            Err(_) => AppSettings::default(),
        };

        let mut app = Self {
            cameras: Vec::new(),
            selected_camera_idx: 0,
            available_formats: Vec::new(),
            selected_format_idx: 0,
            is_recording: false,
            texture: None,
            latest_frame: Arc::new(Mutex::new(None)),
            flip_horizontal: false,
            flip_vertical: false,
            rotation: 0.0,
            pipeline: None,
            recording_pipeline: None,
            record_src: Arc::new(Mutex::new(None)),
            capture_request: Arc::new(Mutex::new(None)),
            settings,
            show_settings: false,
        };

        app.refresh_cameras();

        if !app.cameras.is_empty() {
            app.update_available_formats();
            app.switch_camera(ctx);
        }

        app
    }

    fn refresh_cameras(&mut self) {
        self.cameras.clear();

        let monitor = gst::DeviceMonitor::new();
        monitor.add_filter(Some("Video/Source"), None::<&gst::Caps>);

        if monitor.start().is_ok() {
            for device in monitor.devices() {
                let mut name = device.display_name().to_string();
                if let Some(idx) = name.find(": ") {
                    name = name[..idx].to_string();
                }
                self.cameras.push(CameraInfo { display_name: name, device });
            }
            monitor.stop();
        }
    }

    fn update_available_formats(&mut self) {
        self.available_formats.clear();
        self.selected_format_idx = 0;

        if self.cameras.is_empty() { return; }

        let cam_info = &self.cameras[self.selected_camera_idx];

        let std_res = [
            (3840, 2160), (2560, 1440), (1920, 1080),
            (1600, 1200), (1280, 720), (1024, 768),
            (800, 600), (640, 480), (320, 240)
        ];
        let std_fps = [60, 30, 25, 24, 15, 10, 5];
        let std_fmt = [true, false];

        if let Some(device_caps) = cam_info.device.caps() {
            for &is_mjpeg in &std_fmt {
                for &(w, h) in &std_res {
                    for &fps in &std_fps {
                        let opt = FormatOption { width: w, height: h, fps, is_mjpeg };
                        let opt_caps = opt.to_caps();
                        if device_caps.can_intersect(&opt_caps) {
                            self.available_formats.push(opt);
                        }
                    }
                }
            }
        }

        if self.available_formats.is_empty() {
            self.available_formats.push(FormatOption { width: 1920, height: 1080, fps: 30, is_mjpeg: true });
            self.available_formats.push(FormatOption { width: 1280, height: 720, fps: 30, is_mjpeg: true });
            self.available_formats.push(FormatOption { width: 640, height: 480, fps: 30, is_mjpeg: false });
        }
    }

    fn switch_camera(&mut self, ctx: egui::Context) {
        if self.is_recording {
            if let Some(src) = self.record_src.lock().unwrap().take() {
                let _ = src.end_of_stream();
            }
            if let Some(pipeline) = self.recording_pipeline.take() {
                std::thread::spawn(move || {
                    if let Some(bus) = pipeline.bus() {
                        for msg in bus.iter_timed(gst::ClockTime::NONE) {
                            if let gst::MessageView::Eos(_) = msg.view() { break; }
                        }
                    }
                    let _ = pipeline.set_state(gst::State::Null);
                });
            }
            self.is_recording = false;
        }

        if let Some(pipeline) = self.pipeline.take() {
            let _ = pipeline.set_state(gst::State::Null);
        }

        *self.latest_frame.lock().unwrap() = None;
        self.texture = None;

        if self.cameras.is_empty() || self.available_formats.is_empty() {
            return;
        }

        let cam_info = &self.cameras[self.selected_camera_idx];
        let format_opt = &self.available_formats[self.selected_format_idx];

        let source = match cam_info.device.create_element(Some("source")) {
            Ok(src) => src,
            Err(e) => {
                eprintln!("Failed to create source element: {:?}", e);
                return;
            }
        };

        let capsfilter = gst::ElementFactory::make("capsfilter").build()
        .expect("Missing GStreamer plugin: capsfilter");
        capsfilter.set_property("caps", &format_opt.to_caps());

        let queue1 = gst::ElementFactory::make("queue").build()
        .expect("Missing GStreamer plugin: queue");
        queue1.set_property("max-size-buffers", 2u32);
        queue1.set_property_from_str("leaky", "downstream");

        let decodebin = gst::ElementFactory::make("decodebin").build()
        .expect("Missing GStreamer plugin: decodebin");

        let queue2 = gst::ElementFactory::make("queue").build()
        .expect("Missing GStreamer plugin: queue");
        queue2.set_property("max-size-buffers", 2u32);
        queue2.set_property_from_str("leaky", "downstream");

        let convert = gst::ElementFactory::make("videoconvert").build()
        .expect("Missing GStreamer plugin: videoconvert");

        let appsink = gst_app::AppSink::builder()
        .caps(&gst::Caps::builder("video/x-raw").field("format", "RGB").build())
        .max_buffers(1)
        .drop(true)
        .build();

        appsink.set_property("sync", false);

        let pipeline = gst::Pipeline::with_name("webcam-pipeline");

        pipeline.add_many([
            &source, &capsfilter, &queue1, &decodebin,
            &queue2, &convert, appsink.upcast_ref()
        ]).unwrap();

        gst::Element::link_many([&source, &capsfilter, &queue1, &decodebin]).unwrap();
        gst::Element::link_many([&queue2, &convert, appsink.upcast_ref()]).unwrap();

        let q2_clone = queue2.clone();
        decodebin.connect_pad_added(move |_dbin, src_pad| {
            let sink_pad = q2_clone.static_pad("sink").unwrap();
            if sink_pad.is_linked() { return; }
            if let Err(e) = src_pad.link(&sink_pad) {
                eprintln!("Failed to link decodebin: {:?}", e);
            }
        });

        let frame_clone = self.latest_frame.clone();
        let record_src_clone = self.record_src.clone();
        let capture_request_clone = self.capture_request.clone();

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
            .new_sample(move |appsink| {
                let sample = match appsink.pull_sample() {
                    Ok(s) => s,
                        Err(_) => return Ok(gst::FlowSuccess::Ok),
                };

                let buffer = sample.buffer().unwrap();
                let caps = sample.caps().unwrap();
                let info = gst_video::VideoInfo::from_caps(caps).unwrap();
                let map = buffer.map_readable().unwrap();

                if let Ok(mut src_lock) = record_src_clone.try_lock() {
                    if let Some(src) = src_lock.as_mut() {
                        let _ = src.push_buffer(buffer.to_owned());
                    }
                }

                let mut save_dir = None;
                if let Ok(mut req) = capture_request_clone.try_lock() {
                    if req.is_some() {
                        save_dir = req.take();
                    }
                }

                let image = egui::ColorImage::from_rgb(
                    [info.width() as usize, info.height() as usize],
                                                       map.as_slice(),
                );

                if let Some(dir) = save_dir {
                    let img_clone = image.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = fs::create_dir_all(&dir) {
                            eprintln!("Failed to create capture directory: {}", e);
                            return;
                        }

                        let width = img_clone.width() as u32;
                        let height = img_clone.height() as u32;
                        let raw_data: Vec<u8> = img_clone.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b(), 255]).collect();
                        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

                        let path = std::path::Path::new(&dir).join(format!("capture_{}.png", ts));
                        let filename = path.to_string_lossy().into_owned();

                        if let Err(e) = image::save_buffer(&filename, &raw_data, width, height, image::ColorType::Rgba8) {
                            eprintln!("Failed to save capture: {}", e);
                        } else {
                            println!("Saved capture to {}", filename);
                        }
                    });
                }

                *frame_clone.lock().unwrap() = Some(image);
                ctx.request_repaint();

                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
        );

        if let Err(e) = pipeline.set_state(gst::State::Playing) {
            eprintln!("Failed to start camera pipeline: {:?}", e);
        } else {
            self.pipeline = Some(pipeline);
        }
    }

    fn get_config_path() -> std::path::PathBuf {
        if let Some(proj_dirs) = ProjectDirs::from("com", "yourusername", "gst-cam-rs") {
            let config_dir = proj_dirs.config_dir();
            fs::create_dir_all(config_dir).ok();
            config_dir.join("settings.json")
        } else {
            std::path::PathBuf::from("settings.json")
        }
    }
}

impl eframe::App for WebcamApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top Panel: Action Buttons
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(if self.is_recording { "Stop Recording" } else { "Start Recording" }).clicked() {
                    if !self.is_recording {
                        let fmt = &self.available_formats[self.selected_format_idx];
                        let save_dir = self.settings.save_directory.clone();

                        if let Err(e) = fs::create_dir_all(&save_dir) {
                            eprintln!("Failed to create recording directory: {}", e);
                        } else {
                            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
                            let path = std::path::Path::new(&save_dir).join(format!("video_{}.mp4", ts));
                            let location = path.to_string_lossy().into_owned();

                            // Construct pipeline manually to handle missing plugins and path injections safely
                            let src = gst::ElementFactory::make("appsrc").name("src").build();
                            let conv = gst::ElementFactory::make("videoconvert").build();
                            let enc = gst::ElementFactory::make("x264enc").build();
                            let mux = gst::ElementFactory::make("mp4mux").build();
                            let sink = gst::ElementFactory::make("filesink").build();

                            if let (Ok(src), Ok(conv), Ok(enc), Ok(mux), Ok(sink)) = (src, conv, enc, mux, sink) {
                                let appsrc = src.downcast_ref::<gst_app::AppSrc>().unwrap();

                                let caps = gst::Caps::builder("video/x-raw")
                                .field("format", "RGB")
                                .field("width", fmt.width)
                                .field("height", fmt.height)
                                .field("framerate", gst::Fraction::new(fmt.fps, 1))
                                .build();

                                appsrc.set_caps(Some(&caps));
                                appsrc.set_is_live(true);
                                appsrc.set_do_timestamp(true);
                                appsrc.set_format(gst::Format::Time);

                                enc.set_property_from_str("speed-preset", "superfast");
                                enc.set_property_from_str("tune", "zerolatency");
                                enc.set_property("bitrate", 30_000u32);
                                sink.set_property("location", &location);

                                let pipeline = gst::Pipeline::with_name("recording-pipeline");
                                pipeline.add_many([&src, &conv, &enc, &mux, &sink]).unwrap();
                                gst::Element::link_many([&src, &conv, &enc, &mux, &sink]).unwrap();

                                if let Ok(_) = pipeline.set_state(gst::State::Playing) {
                                    self.recording_pipeline = Some(pipeline);
                                    *self.record_src.lock().unwrap() = Some(appsrc.clone());
                                    self.is_recording = true;
                                    println!("Started recording to {}", location);
                                } else {
                                    eprintln!("Failed to start recording pipeline.");
                                }
                            } else {
                                eprintln!("Cannot record: Missing required GStreamer plugins. Ensure x264enc and mp4mux are installed.");
                            }
                        }
                    } else {
                        if let Some(src) = self.record_src.lock().unwrap().take() {
                            let _ = src.end_of_stream();
                        }
                        if let Some(pipeline) = self.recording_pipeline.take() {
                            std::thread::spawn(move || {
                                if let Some(bus) = pipeline.bus() {
                                    for msg in bus.iter_timed(gst::ClockTime::NONE) {
                                        if let gst::MessageView::Eos(_) = msg.view() { break; }
                                    }
                                }
                                let _ = pipeline.set_state(gst::State::Null);
                                println!("Recording stopped and saved successfully.");
                            });
                        }
                        self.is_recording = false;
                    }
                }

                if ui.button("Capture").clicked() {
                    if let Ok(mut req) = self.capture_request.lock() {
                        *req = Some(self.settings.save_directory.clone());
                    }
                }

                ui.separator();

                if ui.button("Rotate").clicked() {
                    self.rotation = (self.rotation + std::f32::consts::FRAC_PI_2) % (std::f32::consts::PI * 2.0);
                }
                if ui.button("Flip V").clicked() { self.flip_vertical = !self.flip_vertical; }
                if ui.button("Flip H").clicked() { self.flip_horizontal = !self.flip_horizontal; }
            });
            ui.add_space(8.0);
        });

        // Bottom Panel: Config Elements
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let mut camera_changed = false;
                let mut format_changed = false;

                if self.cameras.is_empty() {
                    ui.label("No cameras found");
                } else {
                    ui.label("Camera:");
                    egui::ComboBox::from_id_source("camera_select")
                    .width(200.0)
                    .selected_text(&self.cameras[self.selected_camera_idx].display_name)
                    .show_ui(ui, |ui| {
                        for (i, cam) in self.cameras.iter().enumerate() {
                            if ui.selectable_label(self.selected_camera_idx == i, &cam.display_name).clicked() {
                                self.selected_camera_idx = i;
                                camera_changed = true;
                            }
                        }
                    });

                    ui.add_space(4.0);

                    ui.label("Format:");
                    if !self.available_formats.is_empty() {
                        egui::ComboBox::from_id_source("format_select")
                        .width(180.0)
                        .selected_text(self.available_formats[self.selected_format_idx].display_name())
                        .show_ui(ui, |ui| {
                            for (i, fmt) in self.available_formats.iter().enumerate() {
                                if ui.selectable_label(self.selected_format_idx == i, fmt.display_name()).clicked() {
                                    self.selected_format_idx = i;
                                    format_changed = true;
                                }
                            }
                        });
                    }
                }

                if camera_changed {
                    self.update_available_formats();
                    self.switch_camera(ctx.clone());
                } else if format_changed {
                    self.switch_camera(ctx.clone());
                }

                // Push Settings button to the far right
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ Settings").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                });
            });
            ui.add_space(8.0);
        });

        if self.show_settings {
            let mut show_settings = self.show_settings;
            let mut settings_changed = false;
            let mut new_dir = self.settings.save_directory.clone();

            egui::Window::new("Settings")
            .open(&mut show_settings)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Save Directory:");
                    if ui.text_edit_singleline(&mut new_dir).changed() {
                        settings_changed = true;
                    }
                });
                ui.label("Note: The directory will be created automatically if it doesn't exist.");
            });
            self.show_settings = show_settings;

            if settings_changed {
                self.settings.save_directory = new_dir;
                if let Ok(json) = serde_json::to_string_pretty(&self.settings) {
                    let _ = fs::write(Self::get_config_path(), json);
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Ok(mut frame_lock) = self.latest_frame.lock() {
                if let Some(image) = frame_lock.take() {
                    self.texture = Some(ctx.load_texture("webcam_feed", image, egui::TextureOptions::LINEAR));
                }
            }

            let available_size = ui.available_size();
            let (rect, _response) = ui.allocate_exact_size(available_size, egui::Sense::hover());

            ui.painter().rect_stroke(rect, 4.0, ui.visuals().widgets.noninteractive.bg_stroke);

            ui.allocate_ui_at_rect(rect, |ui| {
                ui.centered_and_justified(|ui| {
                    if let Some(texture) = &self.texture {
                        let aspect_ratio = texture.aspect_ratio();
                        let mut size = rect.size();
                        let is_rotated_90_or_270 = (self.rotation / std::f32::consts::FRAC_PI_2).round() as i32 % 2 != 0;
                        let effective_aspect = if is_rotated_90_or_270 { 1.0 / aspect_ratio } else { aspect_ratio };

                        if size.x / size.y > effective_aspect { size.x = size.y * effective_aspect; }
                        else { size.y = size.x / effective_aspect; }

                        let mut u_min = 0.0; let mut u_max = 1.0;
                        let mut v_min = 0.0; let mut v_max = 1.0;
                        if self.flip_horizontal { std::mem::swap(&mut u_min, &mut u_max); }
                        if self.flip_vertical { std::mem::swap(&mut v_min, &mut v_max); }

                        let img = egui::Image::new(texture)
                        .fit_to_exact_size(if is_rotated_90_or_270 { egui::vec2(size.y, size.x) } else { size })
                        .uv(egui::Rect::from_min_max(egui::pos2(u_min, v_min), egui::pos2(u_max, v_max)))
                        .rotate(self.rotation, egui::Vec2::splat(0.5));
                        ui.add(img);
                    } else {
                        ui.heading("Waiting for camera...");
                    }
                });
            });
        });
    }
}
