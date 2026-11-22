mod video;

use eframe::egui::{self, Align2, Color32, FontId, Rect, TextureHandle, ViewportBuilder};
use serde::Deserialize;
use std::{
    collections::HashMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};
use video::{Audio, Video};

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
}

#[derive(Deserialize)]
struct Config {
    tables: PathBuf,
    executable: PathBuf,
    arguments: Vec<String>,
    ffmpeg: PathBuf,
    #[serde(default = "default_background_video")]
    background_video: PathBuf,
    #[serde(default = "default_scroll_duration_ms")]
    scroll_duration_ms: u32,
    #[serde(default = "default_wheel_idle_seconds")]
    wheel_idle_seconds: u32,
    playfield_monitor: usize,
    backglass_monitor: Option<usize>,
}

fn default_background_video() -> PathBuf {
    "table.original.mp4".into()
}

fn default_scroll_duration_ms() -> u32 {
    375
}

fn default_wheel_idle_seconds() -> u32 {
    5
}

impl Config {
    fn read(path: &Path) -> Result<Self, Box<dyn Error>> {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("Configuration {} : {err}", path.display()))?;
        let mut config: Self = toml::from_str(text.trim_start_matches('\u{feff}'))?;
        let base = path.parent().unwrap_or(Path::new("."));
        config.tables = base.join(config.tables);
        config.executable = base.join(config.executable);
        if config.ffmpeg.components().count() > 1 {
            config.ffmpeg = base.join(config.ffmpeg);
        }
        if !matches!(config.background_video.components().collect::<Vec<_>>().as_slice(), [std::path::Component::Normal(_)]) {
            return Err("background_video doit contenir un nom de fichier dans medias.".into());
        }
        if !config.executable.is_file() {
            return Err(format!("Exécutable introuvable : {}", config.executable.display()).into());
        }
        Ok(config)
    }
}

struct Table {
    name: String,
    file: PathBuf,
    medias: PathBuf,
}

fn scan(root: &Path) -> Result<Vec<Table>, Box<dyn Error>> {
    let mut tables = Vec::new();
    for entry in fs::read_dir(root).map_err(|err| format!("{} : {err}", root.display()))? {
        let directory = entry?.path();
        if !directory.is_dir() {
            continue;
        }
        let name = directory.file_name().unwrap().to_string_lossy().into_owned();
        let exact = directory.join(format!("{name}.vpx"));
        let file = if exact.is_file() {
            exact
        } else {
            let mut candidates = Vec::new();
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vpx")) {
                    candidates.push(path);
                }
            }
            if candidates.len() != 1 {
                if candidates.len() > 1 {
                    eprintln!("Table ignorée (plusieurs fichiers VPX) : {}", directory.display());
                }
                continue;
            }
            candidates.remove(0)
        };
        tables.push(Table { name, file, medias: directory.join("medias") });
    }
    tables.sort_by_cached_key(|table| table.name.to_lowercase());
    if tables.is_empty() {
        return Err("Aucune table VPX trouvée.".into());
    }
    Ok(tables)
}

fn load_png(ctx: &egui::Context, path: &Path, max_size: u32) -> Option<TextureHandle> {
    let image = image::open(path).ok()?;
    let image = if image.width() > max_size || image.height() > max_size {
        image.resize(max_size, max_size, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ctx.load_texture(
        path.to_string_lossy(),
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

fn fitted_size(texture: &TextureHandle, bounds: egui::Vec2) -> egui::Vec2 {
    let size = texture.size_vec2();
    size * (bounds.x / size.x).min(bounds.y / size.y)
}

fn paint_image(painter: &egui::Painter, texture: &TextureHandle, area: Rect, tint: Color32) {
    painter.image(
        texture.id(),
        Rect::from_center_size(area.center(), fitted_size(texture, area.size())),
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        tint,
    );
}

const VIDEO_FADE: Duration = Duration::from_millis(240);
const CACHED_NEIGHBORS: i32 = 4;
const CENTER_WHEEL: egui::Vec2 = egui::vec2(0.31, 0.21);

struct Scroll {
    start: Instant,
    duration: Duration,
    offset: f32,
}

impl Scroll {
    fn offset_at(&self, now: Instant) -> f32 {
        let progress = (now.duration_since(self.start).as_secs_f32()
            / self.duration.as_secs_f32()).min(1.0);
        let eased = progress * progress * (3.0 - 2.0 * progress);
        self.offset * (1.0 - eased)
    }
}

struct Frontend {
    config: Config,
    tables: Vec<Table>,
    selected: usize,
    scroll: Option<Scroll>,
    wheels: HashMap<usize, Option<TextureHandle>>,
    backglass: Option<TextureHandle>,
    video: Option<Video>,
    previous_video: Option<TextureHandle>,
    video_fade_started: Option<Instant>,
    audio: Option<Audio>,
    muted: bool,
    last_activity: Instant,
    preview_at: Option<Instant>,
    game: Option<Child>,
    error: Option<String>,
}

impl Frontend {
    fn index_at(&self, relative: i32) -> usize {
        (self.selected as i64 + i64::from(relative)).rem_euclid(self.tables.len() as i64) as usize
    }

    fn move_selection(&mut self, step: i32, ctx: &egui::Context) {
        let now = Instant::now();
        let remaining = self.scroll.as_ref().map_or(0.0, |scroll| scroll.offset_at(now));
        self.select(self.index_at(step), ctx);
        if self.config.scroll_duration_ms > 0 {
            self.scroll = Some(Scroll {
                start: now,
                duration: Duration::from_millis(u64::from(self.config.scroll_duration_ms)),
                offset: (remaining + step as f32).clamp(-1.25, 1.25),
            });
        }
    }

    fn select(&mut self, index: usize, ctx: &egui::Context) {
        self.selected = index;
        self.scroll = None;
        if let Some(texture) = self.video.take().and_then(|video| video.texture.clone()) {
            self.previous_video = Some(texture);
        }
        self.video_fade_started = None;
        self.audio = None;
        self.last_activity = Instant::now();
        self.error = None;
        self.preview_at = Some(Instant::now() + Duration::from_millis(150));
        self.backglass = self.config.backglass_monitor.and_then(|_| {
            load_png(ctx, &self.tables[index].medias.join("bg.png"), 4096)
        });
        ctx.request_repaint_of(egui::ViewportId::from_hash_of("backglass"));
        let slots = (-CACHED_NEIGHBORS..=CACHED_NEIGHBORS)
            .map(|offset| self.index_at(offset))
            .collect::<Vec<_>>();
        self.wheels.retain(|index, _| slots.contains(index));
        for index in slots {
            self.wheels.entry(index).or_insert_with(|| {
                load_png(ctx, &self.tables[index].medias.join("launcher.png"), 768)
            });
        }
    }

    fn launch(&mut self) {
        let table = &self.tables[self.selected].file;
        let mut command = Command::new(&self.config.executable);
        if let Some(parent) = self.config.executable.parent() {
            command.current_dir(parent);
        }
        for argument in &self.config.arguments {
            if argument == "{table}" {
                command.arg(table);
            } else {
                command.arg(argument);
            }
        }
        match command.spawn() {
            Ok(game) => {
                self.game = Some(game);
                self.video = None;
                self.previous_video = None;
                self.video_fade_started = None;
                self.audio = None;
                self.preview_at = None;
                self.error = None;
            }
            Err(err) => self.error = Some(format!("Impossible de lancer la table : {err}")),
        }
    }

    fn update_preview(&mut self, ctx: &egui::Context, viewport: egui::Vec2) {
        if let Some(deadline) = self.preview_at {
            let now = Instant::now();
            if viewport.x <= 0.0 || viewport.y <= 0.0 {
                ctx.request_repaint_after(Duration::from_millis(16));
            } else if now < deadline {
                ctx.request_repaint_after(deadline - now);
            } else {
                self.preview_at = None;
                self.start_audio();
                let path = self.tables[self.selected].medias.join(&self.config.background_video);
                if path.is_file() {
                    match Video::open(&self.config.ffmpeg, &path, viewport, ctx.clone()) {
                        Ok(video) => self.video = Some(video),
                        Err(err) => self.error = Some(format!("Aperçu vidéo indisponible : {err}")),
                    }
                }
            }
        }
        if let Some(video) = &mut self.video {
            video.update(ctx);
            if video.texture.is_none() && video.has_exited() {
                self.video = None;
                self.error = Some("Aperçu vidéo indisponible.".into());
            }
        }
        if self.previous_video.is_some() && self.video_fade_started.is_none() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn start_audio(&mut self) {
        if self.muted || self.audio.is_some() {
            return;
        }
        let track = self.tables[self.selected].medias.join("audio.mp3");
        if track.is_file() {
            match Audio::open(&self.config.ffmpeg, &track) {
                Ok(audio) => self.audio = Some(audio),
                Err(err) => eprintln!("Audio indisponible ({}): {err}", track.display()),
            }
        }
    }

    fn paint_video(&mut self, painter: &egui::Painter, area: Rect) {
        let current = self.video.as_ref().and_then(|video| video.texture.as_ref());
        let paint = |texture: &TextureHandle, alpha: f32| {
            painter.image(
                texture.id(),
                area,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::from_white_alpha((255.0 * alpha).round() as u8),
            );
        };
        let Some(previous) = &self.previous_video else {
            if let Some(current) = current {
                paint(current, 1.0);
            }
            return;
        };
        if current.is_none() && (self.preview_at.is_some() || self.video.is_some()) {
            paint(previous, 1.0);
            return;
        }
        let started = self.video_fade_started.get_or_insert_with(Instant::now);
        let progress = (started.elapsed().as_secs_f32() / VIDEO_FADE.as_secs_f32()).min(1.0);
        if let Some(current) = current {
            if progress < 1.0 {
                paint(previous, 1.0);
            }
            paint(current, progress);
        } else if progress < 1.0 {
            paint(previous, 1.0 - progress);
        }
        if progress < 1.0 {
            painter.ctx().request_repaint();
        } else {
            self.previous_video = None;
            self.video_fade_started = None;
        }
    }

    fn show_backglass(&self, ctx: &egui::Context) {
        let Some(monitor) = self.config.backglass_monitor else {
            return;
        };
        let texture = self.backglass.clone();
        ctx.show_viewport_deferred(
            egui::ViewportId::from_hash_of("backglass"),
            ViewportBuilder::default()
                .with_title("PinReady Backglass")
                .with_monitor(monitor)
                .with_decorations(false)
                .with_active(false)
                .with_taskbar(false)
                .with_mouse_passthrough(true),
            move |ui, class| {
                if class == egui::ViewportClass::EmbeddedWindow {
                    return;
                }
                let area = ui.max_rect();
                ui.painter().rect_filled(area, 0.0, Color32::BLACK);
                if let Some(texture) = &texture {
                    paint_image(ui.painter(), texture, area, Color32::WHITE);
                }
            },
        );
    }
}

impl eframe::App for Frontend {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let area = ui.max_rect();
        let mut painter = ui.painter().clone();
        painter.rect_filled(area, 0.0, Color32::from_rgb(10, 13, 20));

        if let Some(game) = &mut self.game {
            match game.try_wait() {
                Ok(None) => {
                    ctx.request_repaint_after(Duration::from_millis(250));
                    return;
                }
                Ok(Some(_)) => self.select(self.selected, &ctx),
                Err(err) => {
                    self.select(self.selected, &ctx);
                    self.error = Some(format!("Suivi de VPX impossible : {err}"));
                }
            }
            self.game = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        let (left, right, enter, escape, typing) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
                !input.keys_down.is_empty() || input.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { pressed: true, .. })
                }),
            )
        });
        if typing {
            self.last_activity = Instant::now();
        }
        if escape {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let count = self.tables.len();
        if count > 1 && left != right {
            self.move_selection(if left { -1 } else { 1 }, &ctx);
        }
        if enter {
            self.launch();
            if self.game.is_some() {
                ctx.request_repaint_after(Duration::from_millis(250));
                return;
            }
        }
        self.update_preview(&ctx, area.size());
        self.show_backglass(&ctx);

        self.paint_video(&painter, area);

        let idle = Duration::from_secs(u64::from(self.config.wheel_idle_seconds));
        let remaining = idle.saturating_sub(self.last_activity.elapsed());
        let show_controls = idle.is_zero() || !remaining.is_zero();
        if !remaining.is_zero() {
            ctx.request_repaint_after(remaining);
        }
        let opacity = ctx.animate_bool_with_time(egui::Id::unique("controls-visible"), show_controls, 0.35);
        if opacity <= 0.0 {
            return;
        }
        painter.multiply_opacity(opacity);
        let sound_button = Rect::from_min_size(
            egui::pos2(area.right() - 132.0, area.top() + 16.0),
            egui::vec2(116.0, 32.0),
        );
        let response = ui.scope(|ui| {
            ui.multiply_opacity(opacity);
            let label = if self.muted { "Son coupé" } else { "Son activé" };
            ui.put(sound_button, egui::Button::new(label))
        }).inner;
        if response.clicked() {
            self.last_activity = Instant::now();
            self.muted = !self.muted;
            self.audio = None;
            if self.preview_at.is_none() {
                self.start_audio();
            }
            ctx.request_repaint();
        }

        let unit = area.width().min(area.height() * 1.35);
        let center = egui::pos2(area.center().x, area.top() + area.height() * 0.69);
        let shift = self.scroll.as_ref().map_or(0.0, |scroll| scroll.offset_at(Instant::now()));
        if shift != 0.0 {
            ctx.request_repaint();
        } else {
            self.scroll = None;
        }

        let wheel_height = |relative| {
            self.wheels.get(&self.index_at(relative)).and_then(Option::as_ref)
                .map_or(unit * 0.08, |wheel| fitted_size(wheel, CENTER_WHEEL * unit).y)
        };
        let cursor = -shift;
        let lower = cursor.floor() as i32;
        let height = egui::lerp(wheel_height(lower)..=wheel_height(lower + 1), cursor - lower as f32);
        let half_band = height / 2.0 + 40.0 / ctx.pixels_per_point();
        let band = Rect::from_min_max(
            egui::pos2(area.left(), center.y - half_band),
            egui::pos2(area.right(), center.y + half_band),
        );
        painter.rect_filled(band, 0.0, Color32::from_black_alpha(185));

        let mut slots = (-CACHED_NEIGHBORS..=CACHED_NEIGHBORS)
            .filter(|&relative| count > 1 || relative == 0)
            .map(|relative| (self.index_at(relative), relative as f32 + shift))
            .filter(|&(_, position)| position.abs() < 3.0)
            .collect::<Vec<_>>();
        slots.sort_by(|(_, first), (_, second)| second.abs().total_cmp(&first.abs()));
        for (index, position) in slots {
            let distance = position.abs();
            let near = egui::vec2(0.175, 0.13);
            let far = egui::vec2(0.13, 0.10);
            let size = egui::lerp(CENTER_WHEEL..=near, distance.min(1.0));
            let size = egui::lerp(size..=far, (distance - 1.0).clamp(0.0, 1.0));
            let slot = Rect::from_center_size(
                center + egui::vec2(unit * 0.205 * position, 0.0),
                size * unit,
            );
            let opacity = egui::lerp(255.0..=155.0, distance.min(1.0))
                * (3.0 - distance).clamp(0.0, 1.0);
            let tint = Color32::from_white_alpha(opacity.round() as u8);
            if let Some(Some(wheel)) = self.wheels.get(&index) {
                paint_image(&painter, wheel, slot, tint);
            } else {
                painter.with_clip_rect(slot).text(
                    slot.center(), Align2::CENTER_CENTER, &self.tables[index].name,
                    FontId::proportional((unit * 0.023).clamp(13.0, 28.0)), tint,
                );
            }
        }
        painter.text(
            egui::pos2(center.x, (band.bottom() + 6.0).min(area.bottom() - 45.0)),
            Align2::CENTER_TOP, &self.tables[self.selected].name,
            FontId::proportional((unit * 0.029).clamp(14.0, 34.0)), Color32::WHITE,
        );
        painter.text(
            egui::pos2(center.x, area.bottom() - 16.0), Align2::CENTER_BOTTOM,
            self.error.as_deref().unwrap_or("← / →  Choisir     Entrée  Jouer     Échap  Quitter"),
            FontId::proportional(16.0), Color32::from_gray(210),
        );
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let path = std::env::args_os().nth(1).map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?.with_file_name("config.toml"));
    let path = std::path::absolute(path)?;
    let config = Config::read(&path)?;
    #[cfg(windows)]
    let config = {
        let mut config = config;
        let monitors = unsafe { GetSystemMetrics(80) }.max(1) as usize; // SM_CMONITORS
        if config.playfield_monitor >= monitors {
            return Err(format!("Écran playfield introuvable ({} écran(s) détecté(s)).", monitors).into());
        }
        config.backglass_monitor = config.backglass_monitor.filter(|&index| index < monitors);
        config
    };
    let backglass_monitor = config.backglass_monitor.filter(|&index| index != config.playfield_monitor);
    let config = Config { backglass_monitor, ..config };
    let tables = scan(&config.tables)?;
    let native = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("PinReady")
            .with_monitor(config.playfield_monitor)
            .with_decorations(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("PinReady", native, Box::new(move |creation| {
        let mut app = Frontend {
            config, tables, selected: 0, scroll: None, wheels: HashMap::new(), backglass: None,
            video: None, previous_video: None, video_fade_started: None,
            audio: None, muted: false, last_activity: Instant::now(),
            preview_at: None, game: None, error: None,
        };
        app.select(0, &creation.egui_ctx);
        Ok(Box::new(app))
    }))?;
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}