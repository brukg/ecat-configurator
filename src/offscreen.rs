//! Off-screen picture mode: runs the user interface without opening a window and
//! rasterises egui's triangles in memory, then saves a PNG.

use crate::ui::{App, StartOptions};
use eframe::egui::{self, epaint::ImageData, Color32, Pos2, Rect, TextureId};
use std::collections::HashMap;
use std::path::Path;

struct Tex {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Tex {
    /// Bilinear sample, premultiplied RGBA in 0..1.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u * self.w as f32 - 0.5;
        let y = v * self.h as f32 - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let get = |xi: i64, yi: i64| -> [f32; 4] {
            let xi = xi.clamp(0, self.w as i64 - 1) as usize;
            let yi = yi.clamp(0, self.h as i64 - 1) as usize;
            self.px[yi * self.w + xi]
        };
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = get(x0, y0);
        let b = get(x0 + 1, y0);
        let c = get(x0, y0 + 1);
        let d = get(x0 + 1, y0 + 1);
        let mut o = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            o[k] = top + (bot - top) * fy;
        }
        o
    }
}

fn to_f(c: Color32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
}

fn apply_textures(textures: &mut HashMap<TextureId, Tex>, delta: &egui::TexturesDelta) {
    for (id, d) in delta.set.iter() {
        let (w, h) = (d.image.width(), d.image.height());
        let px: Vec<[f32; 4]> = match &d.image {
            ImageData::Color(img) => img.pixels.iter().map(|c| to_f(*c)).collect(),
            ImageData::Font(f) => f.srgba_pixels(None).map(to_f).collect(),
        };
        match d.pos {
            None => {
                textures.insert(*id, Tex { w, h, px });
            }
            Some([x, y]) => {
                if let Some(t) = textures.get_mut(id) {
                    for row in 0..h {
                        for col in 0..w {
                            let (tx, ty) = (x + col, y + row);
                            if tx < t.w && ty < t.h {
                                t.px[ty * t.w + tx] = px[row * w + col];
                            }
                        }
                    }
                }
            }
        }
    }
}

fn edge(a: Pos2, b: Pos2, p: (f32, f32)) -> f32 {
    (b.x - a.x) * (p.1 - a.y) - (b.y - a.y) * (p.0 - a.x)
}

pub fn screenshot(opts: StartOptions, path: &Path, w: u32, h: u32, frames: u32) -> Result<(), String> {
    let ctx = egui::Context::default();
    // extra windows are drawn inside the picture; nothing is ever opened on screen
    ctx.set_embed_viewports(true);
    let mut app = App::new(&ctx, opts);
    let mut textures: HashMap<TextureId, Tex> = HashMap::new();
    let mut prims = Vec::new();
    let mut ppp = 1.0;
    let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(w as f32, h as f32));
    // test hook: ECAT_TEST_CLICKS="frame:x:y,frame:key:Z:ctrl,frame:copy,frame:paste" replays input
    enum Act {
        Click(f32, f32),
        Key(egui::Key, egui::Modifiers),
        Copy,
        Paste,
    }
    let acts: Vec<(u32, Act)> = std::env::var("ECAT_TEST_CLICKS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|c| {
            let v: Vec<&str> = c.split(':').map(|x| x.trim()).collect();
            let f: u32 = v.first()?.parse().ok()?;
            match v.get(1).copied() {
                Some("key") => {
                    let key = egui::Key::from_name(v.get(2)?)?;
                    let mut m = egui::Modifiers::default();
                    for flag in v.iter().skip(3) {
                        match *flag {
                            "ctrl" => {
                                m.ctrl = true;
                                m.command = true;
                            }
                            "shift" => m.shift = true,
                            _ => {}
                        }
                    }
                    Some((f, Act::Key(key, m)))
                }
                Some("copy") => Some((f, Act::Copy)),
                Some("paste") => Some((f, Act::Paste)),
                Some(x) => Some((f, Act::Click(x.parse().ok()?, v.get(2)?.parse().ok()?))),
                None => None,
            }
        })
        .collect();
    let frames = frames.max(acts.iter().map(|c| c.0 + 30).max().unwrap_or(0));
    for f in 0..frames.max(2) {
        let mut events = Vec::new();
        let mut modifiers = egui::Modifiers::default();
        for (cf, act) in acts.iter() {
            match act {
                Act::Click(x, y) => {
                    let pos = Pos2::new(*x, *y);
                    let press = |pressed| egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    };
                    if f + 1 == *cf {
                        events.push(egui::Event::PointerMoved(pos));
                    } else if f == *cf {
                        events.push(egui::Event::PointerMoved(pos));
                        events.push(press(true));
                    } else if f == cf + 1 {
                        events.push(press(false));
                    }
                }
                Act::Key(key, m) => {
                    if f == *cf || f == cf + 1 {
                        modifiers = *m;
                        events.push(egui::Event::Key {
                            key: *key,
                            physical_key: None,
                            pressed: f == *cf,
                            repeat: false,
                            modifiers: *m,
                        });
                    }
                }
                Act::Copy if f == *cf => events.push(egui::Event::Copy),
                Act::Paste if f == *cf => events.push(egui::Event::Paste(String::new())),
                _ => {}
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(f as f64 / 60.0),
            events,
            modifiers,
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| app.ui(ctx));
        apply_textures(&mut textures, &out.textures_delta);
        if f + 1 == frames.max(2) {
            ppp = out.pixels_per_point;
            prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        }
        for id in out.textures_delta.free.iter() {
            textures.remove(id);
        }
    }

    let (wu, hu) = (w as usize, h as usize);
    let bg = to_f(ctx.style().visuals.panel_fill);
    let mut fb: Vec<[f32; 4]> = vec![[bg[0], bg[1], bg[2], 1.0]; wu * hu];
    for cp in prims.iter() {
        let mesh = match &cp.primitive {
            egui::epaint::Primitive::Mesh(m) => m,
            _ => continue,
        };
        let tex = match textures.get(&mesh.texture_id) {
            Some(t) => t,
            None => continue,
        };
        let clip = cp.clip_rect;
        let cx0 = (clip.min.x * ppp).max(0.0);
        let cy0 = (clip.min.y * ppp).max(0.0);
        let cx1 = (clip.max.x * ppp).min(w as f32);
        let cy1 = (clip.max.y * ppp).min(h as f32);
        if cx0 >= cx1 || cy0 >= cy1 {
            continue;
        }
        for tri in mesh.indices.chunks_exact(3) {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let p: Vec<Pos2> = v.iter().map(|x| Pos2::new(x.pos.x * ppp, x.pos.y * ppp)).collect();
            let area = edge(p[0], p[1], (p[2].x, p[2].y));
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = p.iter().map(|q| q.x).fold(f32::MAX, f32::min).max(cx0).floor() as usize;
            let maxx = p.iter().map(|q| q.x).fold(f32::MIN, f32::max).min(cx1).ceil() as usize;
            let miny = p.iter().map(|q| q.y).fold(f32::MAX, f32::min).max(cy0).floor() as usize;
            let maxy = p.iter().map(|q| q.y).fold(f32::MIN, f32::max).min(cy1).ceil() as usize;
            let cols = [to_f(v[0].color), to_f(v[1].color), to_f(v[2].color)];
            for y in miny..maxy.min(hu) {
                let py = y as f32 + 0.5;
                if py < cy0 || py > cy1 {
                    continue;
                }
                for x in minx..maxx.min(wu) {
                    let px = x as f32 + 0.5;
                    if px < cx0 || px > cx1 {
                        continue;
                    }
                    let w0 = edge(p[1], p[2], (px, py)) / area;
                    let w1 = edge(p[2], p[0], (px, py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let t = tex.sample(u, vv);
                    let mut src = [0.0f32; 4];
                    for k in 0..4 {
                        src[k] = (w0 * cols[0][k] + w1 * cols[1][k] + w2 * cols[2][k]) * t[k];
                    }
                    let d = &mut fb[y * wu + x];
                    let ia = 1.0 - src[3];
                    for k in 0..4 {
                        d[k] = src[k] + d[k] * ia;
                    }
                }
            }
        }
    }
    let mut buf = Vec::with_capacity(wu * hu * 4);
    for c in fb.iter() {
        for k in 0..3 {
            buf.push((c[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        buf.push(255);
    }
    if let Some(d) = path.parent() {
        if !d.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(d);
        }
    }
    image::save_buffer(path, &buf, w, h, image::ColorType::Rgba8).map_err(|e| e.to_string())
}
