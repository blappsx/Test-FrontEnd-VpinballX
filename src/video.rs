use eframe::egui;
use std::{
    io::{self, Read},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

#[derive(Default)]
struct Frames {
    latest: Option<Vec<u8>>,
    spare: Option<Vec<u8>>,
}

pub struct Video {
    child: Child,
    size: [usize; 2],
    frames: Arc<Mutex<Frames>>,
    pub texture: Option<egui::TextureHandle>,
}

impl Video {
    pub fn open(
        ffmpeg: &Path,
        path: &Path,
        viewport: egui::Vec2,
        ctx: egui::Context,
    ) -> io::Result<Self> {
        let pixels_per_point = ctx.pixels_per_point();
        let even = |edge: f32| ((edge * pixels_per_point / 2.0).round() as usize).max(1) * 2;
        let size = [even(viewport.x), even(viewport.y)];
        let [width, height] = size;
        let filter = format!(
            "fps=24,scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2:reset_sar=1,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,format=rgb24"
        );
        let mut child = Command::new(ffmpeg)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .args(["-nostdin", "-loglevel", "error", "-re", "-stream_loop", "-1", "-i"])
            .arg(path)
            .args([
                "-map", "0:v:0", "-an", "-vf", &filter, "-pix_fmt", "rgba",
                "-c:v", "rawvideo", "-threads:v", "1", "-f", "rawvideo", "pipe:1",
            ])
            .stdout(Stdio::piped())
            .spawn()?;

        eprintln!("Vidéo de fond : {} (rendu {width} × {height})", path.display());
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let frames = Arc::new(Mutex::new(Frames::default()));
        let shared = Arc::clone(&frames);
        thread::spawn(move || {
            let bytes = width * height * 4;
            let mut buffer = vec![0; bytes];
            while stdout.read_exact(&mut buffer).is_ok() {
                let reusable = {
                    let mut frames = shared.lock().unwrap();
                    frames.latest.replace(buffer).or_else(|| frames.spare.take())
                };
                ctx.request_repaint_of(egui::ViewportId::ROOT);
                buffer = reusable.unwrap_or_else(|| vec![0; bytes]);
            }
        });

        Ok(Self { child, size, frames, texture: None })
    }

    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        let Some(frame) = self.frames.lock().unwrap().latest.take() else {
            return;
        };
        // FFmpeg supplies opaque RGBA frames for the playfield videos.
        let pixels = egui::ColorImage::from_rgba_premultiplied(self.size, &frame);
        self.frames.lock().unwrap().spare = Some(frame);
        if let Some(texture) = &mut self.texture {
            texture.set(pixels, egui::TextureOptions::LINEAR);
        } else {
            self.texture = Some(ctx.load_texture("background-video", pixels, egui::TextureOptions::LINEAR));
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub struct Audio(Child);

impl Audio {
    pub fn open(ffmpeg: &Path, track: &Path) -> io::Result<Self> {
        let player = ffmpeg.with_file_name("ffplay.exe");
        Command::new(player)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .args(["-loglevel", "error", "-nodisp", "-vn", "-loop", "0"])
            .arg(track)
            .stdin(Stdio::null())
            .spawn()
            .map(Self)
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
