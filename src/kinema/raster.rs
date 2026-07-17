//! A tiny software rasterizer: an RGB canvas with alpha-blended primitives.
//!
//! Deterministic and dependency-free. Coordinates are in pixels with the origin
//! at the top-left. Primitives composite over the existing canvas using
//! src-over alpha so overlapping objects and fades render correctly.

use super::brief::Color;
use super::png;

/// A fixed-size RGB raster surface.
pub struct Canvas {
    width: u32,
    height: u32,
    /// Row-major RGB, `width*height*3` bytes.
    pixels: Vec<u8>,
}

impl Canvas {
    /// Create a canvas filled with a solid background colour.
    pub fn new(width: u32, height: u32, background: Color) -> Self {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
        for _ in 0..(width as usize * height as usize) {
            pixels.push(background.r);
            pixels.push(background.g);
            pixels.push(background.b);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Blend a single pixel with src-over alpha.
    fn blend_pixel(&mut self, x: i64, y: i64, color: Color, alpha: f64) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 || alpha <= 0.0 {
            return;
        }
        let idx = ((y as usize) * self.width as usize + x as usize) * 3;
        let bg = Color {
            r: self.pixels[idx],
            g: self.pixels[idx + 1],
            b: self.pixels[idx + 2],
        };
        let blended = color.over(bg, alpha);
        self.pixels[idx] = blended.r;
        self.pixels[idx + 1] = blended.g;
        self.pixels[idx + 2] = blended.b;
    }

    /// Fill an axis-aligned rectangle centred on `(cx, cy)`.
    pub fn fill_rect(&mut self, cx: f64, cy: f64, w: f64, h: f64, color: Color, alpha: f64) {
        let x0 = (cx - w / 2.0).floor() as i64;
        let x1 = (cx + w / 2.0).ceil() as i64;
        let y0 = (cy - h / 2.0).floor() as i64;
        let y1 = (cy + h / 2.0).ceil() as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend_pixel(x, y, color, alpha);
            }
        }
    }

    /// Fill a circle centred on `(cx, cy)` with the given radius. The edge is
    /// lightly anti-aliased by attenuating alpha over the last pixel of radius.
    pub fn fill_circle(&mut self, cx: f64, cy: f64, radius: f64, color: Color, alpha: f64) {
        if radius <= 0.0 {
            return;
        }
        let x0 = (cx - radius - 1.0).floor() as i64;
        let x1 = (cx + radius + 1.0).ceil() as i64;
        let y0 = (cy - radius - 1.0).floor() as i64;
        let y1 = (cy + radius + 1.0).ceil() as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                // Coverage: 1 inside, ramps to 0 across the outermost pixel.
                let coverage = (radius - dist + 0.5).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend_pixel(x, y, color, alpha * coverage);
                }
            }
        }
    }

    /// Draw a thick line segment as a series of stamped discs.
    pub fn stroke_line(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        thickness: f64,
        color: Color,
        alpha: f64,
    ) {
        let (x0, y0) = start;
        let (x1, y1) = end;
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let steps = len.ceil() as i64;
        let r = (thickness / 2.0).max(0.5);
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            self.fill_circle(x0 + dx * t, y0 + dy * t, r, color, alpha);
        }
    }

    /// Encode the current canvas as a PNG byte stream.
    pub fn to_png(&self) -> Vec<u8> {
        png::encode_rgb(self.width, self.height, &self.pixels)
    }

    /// Borrow the raw RGB pixel buffer (row-major).
    #[cfg(test)]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(c: &Canvas, x: u32, y: u32) -> Color {
        let idx = ((y * c.width() + x) * 3) as usize;
        let p = c.pixels();
        Color {
            r: p[idx],
            g: p[idx + 1],
            b: p[idx + 2],
        }
    }

    #[test]
    fn new_canvas_is_solid_background() {
        let bg = Color {
            r: 10,
            g: 20,
            b: 30,
        };
        let c = Canvas::new(4, 4, bg);
        assert_eq!(pixel(&c, 0, 0), bg);
        assert_eq!(pixel(&c, 3, 3), bg);
        assert_eq!(c.pixels().len(), 4 * 4 * 3);
    }

    #[test]
    fn fill_rect_paints_center() {
        let mut c = Canvas::new(10, 10, Color { r: 0, g: 0, b: 0 });
        c.fill_rect(5.0, 5.0, 4.0, 4.0, Color::WHITE, 1.0);
        assert_eq!(pixel(&c, 5, 5), Color::WHITE);
        // A far corner remains background.
        assert_eq!(pixel(&c, 0, 0), Color { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn fill_circle_paints_center_not_corner() {
        let mut c = Canvas::new(20, 20, Color { r: 0, g: 0, b: 0 });
        c.fill_circle(10.0, 10.0, 4.0, Color::WHITE, 1.0);
        assert_eq!(pixel(&c, 10, 10), Color::WHITE);
        assert_eq!(pixel(&c, 0, 0), Color { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn out_of_bounds_draw_is_ignored() {
        let mut c = Canvas::new(4, 4, Color { r: 0, g: 0, b: 0 });
        // Entirely off-canvas — must not panic and must leave pixels untouched.
        c.fill_circle(-50.0, -50.0, 3.0, Color::WHITE, 1.0);
        assert_eq!(pixel(&c, 0, 0), Color { r: 0, g: 0, b: 0 });
    }

    #[test]
    fn stroke_line_paints_along_path() {
        let mut c = Canvas::new(20, 20, Color { r: 0, g: 0, b: 0 });
        c.stroke_line((2.0, 2.0), (17.0, 2.0), 3.0, Color::WHITE, 1.0);
        // The line is anti-aliased, so the centre pixel is strongly (but not
        // necessarily fully) painted.
        assert!(pixel(&c, 10, 2).r > 200, "line should paint the mid pixel");
    }

    #[test]
    fn to_png_starts_with_signature() {
        let c = Canvas::new(2, 2, Color::WHITE);
        let png = c.to_png();
        assert_eq!(
            &png[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
    }
}
