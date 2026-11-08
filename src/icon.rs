// Procedural app icon. build.rs includes this file for the .exe icon and the
// tray uses it at run time, so both always match.

/// Distance from `(px, py)` to a rounded rectangle. Negative inside.
fn round_rect(px: f32, py: f32, cx: f32, cy: f32, half_w: f32, half_h: f32, radius: f32) -> f32 {
    let qx = (px - cx).abs() - (half_w - radius);
    let qy = (py - cy).abs() - (half_h - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// RGBA pixels of a square icon: a blue tile with three white bars.
pub fn rgba(size: u32) -> Vec<u8> {
    const BG: [f32; 3] = [108.0, 140.0, 255.0];
    // (center x, half height) of each bar. Everything is in 0..1 units.
    const BARS: [(f32, f32); 3] = [(0.32, 0.13), (0.5, 0.25), (0.68, 0.18)];
    const BAR_HALF_WIDTH: f32 = 0.055;

    let scale = size as f32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
            let tile = round_rect(u, v, 0.5, 0.5, 0.46, 0.46, 0.2);
            let tile_cover = (0.5 - tile * scale).clamp(0.0, 1.0);
            let bar_cover = BARS
                .iter()
                .map(|&(cx, half_h)| {
                    let d = round_rect(u, v, cx, 0.5, BAR_HALF_WIDTH, half_h, BAR_HALF_WIDTH);
                    (0.5 - d * scale).clamp(0.0, 1.0)
                })
                .fold(0.0f32, f32::max);
            for channel in BG {
                pixels.push((channel + (255.0 - channel) * bar_cover) as u8);
            }
            pixels.push((tile_cover * 255.0) as u8);
        }
    }
    pixels
}
