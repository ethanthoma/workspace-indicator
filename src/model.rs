use crate::Result;
use serde::Deserialize;
use std::{ffi::OsStr, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub display_ms: u64,
    pub fade_ms: u64,
    pub margin: i32,
    pub slots: u32,
    pub background: String,
    pub active: String,
    pub occupied: String,
    pub empty: String,
}

impl Config {
    pub fn load(path: &OsStr) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > 4096 {
            return Err("configuration exceeds 4096 bytes".into());
        }
        let config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        config.validate()?;
        Ok(config)
    }
    fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.slots) {
            return Err("slots must be between 1 and 32".into());
        }
        if !(1..=10000).contains(&self.display_ms) {
            return Err("display_ms must be between 1 and 10000".into());
        }
        if !(1..=2000).contains(&self.fade_ms) {
            return Err("fade_ms must be between 1 and 2000".into());
        }
        if !(0..=1000).contains(&self.margin) {
            return Err("margin must be between 0 and 1000".into());
        }
        for color in [&self.background, &self.active, &self.occupied, &self.empty] {
            parse_color(color)?;
        }
        Ok(())
    }
    pub fn width(&self) -> u32 {
        32 + self.slots * 20
    }
}

#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub monitor: String,
    pub active: i32,
    pub occupied: [bool; 32],
}
impl Snapshot {
    pub fn focus(&self) -> (&str, i32) {
        (&self.monitor, self.active)
    }
}

pub fn opacity(elapsed: Duration, config: &Config) -> f32 {
    let fade_elapsed = elapsed.saturating_sub(Duration::from_millis(config.display_ms));
    (1.0 - fade_elapsed.as_secs_f32() / Duration::from_millis(config.fade_ms).as_secs_f32())
        .clamp(0.0, 1.0)
}

fn parse_color(value: &str) -> Result<[u8; 3]> {
    let hex = value.strip_prefix('#').ok_or("colors must use #RRGGBB")?;
    if hex.len() != 6 {
        return Err("colors must use #RRGGBB".into());
    }
    let number = u32::from_str_radix(hex, 16)?;
    Ok([(number >> 16) as u8, (number >> 8) as u8, number as u8])
}

pub fn render(canvas: &mut [u8], scale: u32, opacity: f32, config: &Config, snapshot: &Snapshot) {
    let colors = [
        &config.background,
        &config.active,
        &config.occupied,
        &config.empty,
    ]
    .map(|color| parse_color(color).expect("validated color"));
    let width = config.width() * scale;
    assert_eq!(canvas.len(), (width * 40 * scale * 4) as usize);
    for (index, pixel) in canvas.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (index as u32 % width) as f32 / scale as f32 + 0.5 / scale as f32;
        let y = (index as u32 / width) as f32 / scale as f32 + 0.5 / scale as f32;
        let center_x = x.clamp(20.0, config.width() as f32 - 20.0);
        let distance = ((x - center_x).powi(2) + (y - 20.0).powi(2)).sqrt() - 19.0;
        let coverage = (0.5 - distance * scale as f32).clamp(0.0, 1.0);
        let mut color = colors[0];
        let mut alpha = coverage * 0.94;
        for slot in 0..config.slots {
            let active = snapshot.active == (slot + 1) as i32;
            let radius = if active { 5.0 } else { 3.5 };
            let distance =
                ((x - (26 + slot * 20) as f32).powi(2) + (y - 20.0).powi(2)).sqrt() - radius;
            let dot = (0.5 - distance * scale as f32).clamp(0.0, 1.0);
            if dot > 0.0 {
                let foreground = colors[if active {
                    1
                } else if snapshot.occupied[slot as usize] {
                    2
                } else {
                    3
                }];
                for channel in 0..3 {
                    color[channel] = (color[channel] as f32 * (1.0 - dot)
                        + foreground[channel] as f32 * dot)
                        as u8;
                }
                alpha += (1.0 - alpha) * dot;
            }
        }
        alpha *= opacity;
        let argb = ((alpha * 255.0).round() as u32) << 24
            | ((color[0] as f32 * alpha).round() as u32) << 16
            | ((color[1] as f32 * alpha).round() as u32) << 8
            | (color[2] as f32 * alpha).round() as u32;
        pixel.copy_from_slice(&argb.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            slots: 5,
            display_ms: 900,
            fade_ms: 200,
            margin: 60,
            background: "#191724".into(),
            active: "#c4a7e7".into(),
            occupied: "#e0def4".into(),
            empty: "#524f67".into(),
        }
    }
    #[test]
    fn fade_is_bounded_monotonic_and_expires() {
        let config = config();
        let mut previous = 1.0;
        for milliseconds in 0..=2000 {
            let alpha = opacity(Duration::from_millis(milliseconds), &config);
            assert!((0.0..=previous).contains(&alpha));
            if milliseconds <= config.display_ms {
                assert_eq!(alpha, 1.0);
            }
            if milliseconds >= config.display_ms + config.fade_ms {
                assert_eq!(alpha, 0.0);
            }
            previous = alpha;
        }
    }
    #[test]
    fn rendered_pixels_are_premultiplied_at_every_supported_size_and_scale() {
        let mut config = config();
        for slots in 1..=32 {
            config.slots = slots;
            for scale in 1..=4 {
                let mut canvas = vec![0; (config.width() * 40 * scale * scale * 4) as usize];
                let snapshot = Snapshot {
                    monitor: "DP-2".into(),
                    active: slots as i32,
                    occupied: [true; 32],
                };
                for alpha in [0.0, 0.5, 1.0] {
                    render(&mut canvas, scale, alpha, &config, &snapshot);
                    for pixel in canvas.as_chunks::<4>().0 {
                        let argb = u32::from_ne_bytes(*pixel);
                        let alpha_byte = argb >> 24;
                        for shift in [0, 8, 16] {
                            assert!((argb >> shift) & 255 <= alpha_byte);
                        }
                        if alpha == 0.0 {
                            assert_eq!(argb, 0);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn rejects_invalid_configuration() {
        let mut config = config();
        config.slots = 0;
        assert!(config.validate().is_err());
        for color in ["", "ffffff", "#12345", "#gggggg", "#éabcd"] {
            assert!(parse_color(color).is_err());
        }
    }
}
