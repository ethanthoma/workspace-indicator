use crate::Result;
use serde::Deserialize;
use std::{ffi::OsStr, io::Read, time::Duration};

const SLOT_SPACING_PIXELS: u32 = 20;
const SLOT_CENTER_FIRST_PIXELS: u32 = 26;
const WIDTH_PADDING_PIXELS: u32 = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub animation_ms: u64,
    pub margin_top: i32,
    pub height: u32,
    pub slots: u32,
    pub active: String,
    pub occupied: String,
    pub empty: String,
    pub urgent: String,
}

impl Configuration {
    pub fn load(path: &OsStr) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(4097)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err("configuration exceeds 4096 bytes".into());
        }
        let configuration: Self = serde_json::from_slice(&bytes)?;
        configuration.validate()?;
        Ok(configuration)
    }
    fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.slots) {
            return Err("slots must be between 1 and 32".into());
        }
        if !(1..=2000).contains(&self.animation_ms) {
            return Err("animation_ms must be between 1 and 2000".into());
        }
        if !(24..=64).contains(&self.height) {
            return Err("height must be between 24 and 64".into());
        }
        if !(0..=1000).contains(&self.margin_top) {
            return Err("margin_top must be between 0 and 1000".into());
        }
        for color in [&self.active, &self.occupied, &self.empty, &self.urgent] {
            parse_color(color)?;
        }
        Ok(())
    }
    pub fn width(&self) -> u32 {
        WIDTH_PADDING_PIXELS + self.slots * SLOT_SPACING_PIXELS
    }
    pub fn active_center(&self, workspace: i32) -> Option<f32> {
        (1..=self.slots as i32)
            .contains(&workspace)
            .then(|| slot_center((workspace - 1) as u32))
    }
}

#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub monitor: String,
    pub active: i32,
    pub occupied: [bool; 32],
    pub urgent: [bool; 32],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub left: f32,
    pub right: f32,
}

pub struct Motion {
    source: Capsule,
    target: f32,
    started_at: Duration,
    duration: Duration,
}

impl Motion {
    pub fn new(target: f32, duration: Duration) -> Self {
        Self {
            source: Capsule {
                left: target,
                right: target,
            },
            target,
            started_at: Duration::ZERO,
            duration,
        }
    }
    pub fn retarget(&mut self, target: f32, now: Duration) {
        if self.target == target {
            return;
        }
        self.source = self.sample(now);
        self.target = target;
        self.started_at = now;
    }
    pub fn running(&self, now: Duration) -> bool {
        if now.saturating_sub(self.started_at) >= self.duration {
            return false;
        }
        self.source
            != Capsule {
                left: self.target,
                right: self.target,
            }
    }
    pub fn sample(&self, now: Duration) -> Capsule {
        let progress = (now.saturating_sub(self.started_at).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let leading = 1.0 - (1.0 - progress).powi(3);
        let trailing = progress.powi(3);
        let moving_right = self.target >= (self.source.left + self.source.right) * 0.5;
        let left_progress = if moving_right { trailing } else { leading };
        let right_progress = if moving_right { leading } else { trailing };
        let left = self.source.left + (self.target - self.source.left) * left_progress;
        let right = self.source.right + (self.target - self.source.right) * right_progress;
        Capsule {
            left: left.min(right),
            right: left.max(right),
        }
    }
}

fn parse_color(value: &str) -> Result<[u8; 3]> {
    let hex = value.strip_prefix('#').ok_or("colors must use #RRGGBB")?;
    if hex.len() != 6 {
        return Err("colors must use #RRGGBB".into());
    }
    let number = u32::from_str_radix(hex, 16)?;
    Ok([(number >> 16) as u8, (number >> 8) as u8, number as u8])
}

fn slot_center(slot_index: u32) -> f32 {
    SLOT_CENTER_FIRST_PIXELS as f32 + slot_index as f32 * SLOT_SPACING_PIXELS as f32
}

fn coverage(x: f32, y: f32, capsule: Capsule, radius: f32, scale: u32) -> f32 {
    let center_x = x.clamp(capsule.left, capsule.right);
    let distance = ((x - center_x).powi(2) + y.powi(2)).sqrt() - radius;
    (0.5 - distance * scale as f32).clamp(0.0, 1.0)
}

pub fn render(
    canvas: &mut [u8],
    scale: u32,
    capsule: Option<Capsule>,
    configuration: &Configuration,
    snapshot: &Snapshot,
) {
    let active_color = parse_color(&configuration.active).expect("validated active color");
    let occupied_color = parse_color(&configuration.occupied).expect("validated occupied color");
    let empty_color = parse_color(&configuration.empty).expect("validated empty color");
    let urgent_color = parse_color(&configuration.urgent).expect("validated urgent color");
    let width = configuration.width() * scale;
    assert_eq!(
        canvas.len(),
        (width * configuration.height * scale * 4) as usize
    );
    for (index, pixel) in canvas.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (index as u32 % width) as f32 / scale as f32 + 0.5 / scale as f32;
        let y = (index as u32 / width) as f32 / scale as f32 + 0.5 / scale as f32
            - configuration.height as f32 * 0.5;
        let mut alpha = 0.0;
        let mut premultiplied = [0.0; 3];
        let mut composite = |color: [u8; 3], coverage: f32| {
            for channel in 0..3 {
                premultiplied[channel] = color[channel] as f32 / 255.0 * coverage
                    + premultiplied[channel] * (1.0 - coverage);
            }
            alpha = coverage + alpha * (1.0 - coverage);
        };
        for slot_index in 0..configuration.slots {
            let center = slot_center(slot_index);
            let dot = coverage(
                x,
                y,
                Capsule {
                    left: center,
                    right: center,
                },
                3.5,
                scale,
            );
            let color = if snapshot.urgent[slot_index as usize] {
                urgent_color
            } else if snapshot.occupied[slot_index as usize] {
                occupied_color
            } else {
                empty_color
            };
            composite(color, dot);
        }
        if let Some(capsule) = capsule {
            composite(active_color, coverage(x, y, capsule, 5.0, scale));
        }
        let argb = ((alpha * 255.0).round() as u32) << 24
            | ((premultiplied[0] * 255.0).round() as u32) << 16
            | ((premultiplied[1] * 255.0).round() as u32) << 8
            | (premultiplied[2] * 255.0).round() as u32;
        pixel.copy_from_slice(&argb.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configuration() -> Configuration {
        Configuration {
            slots: 5,
            animation_ms: 320,
            height: 30,
            margin_top: 0,
            active: "#e0e0e0".into(),
            occupied: "#8f93a2".into(),
            empty: "#4b526d".into(),
            urgent: "#f2283c".into(),
        }
    }
    #[test]
    fn motion_stretches_settles_and_remains_bounded_in_both_directions() {
        for source in 0..32 {
            for target in 0..32 {
                let source = 26.0 + source as f32 * 20.0;
                let target = 26.0 + target as f32 * 20.0;
                let mut motion = Motion::new(source, Duration::from_millis(320));
                motion.retarget(target, Duration::ZERO);
                for milliseconds in 0..=400 {
                    let shape = motion.sample(Duration::from_millis(milliseconds));
                    assert!(shape.left >= source.min(target));
                    assert!(shape.left <= shape.right);
                    assert!(shape.right <= source.max(target));
                    if milliseconds >= 320 {
                        assert_eq!(
                            shape,
                            Capsule {
                                left: target,
                                right: target
                            }
                        );
                    }
                }
                let midpoint = motion.sample(Duration::from_millis(160));
                assert!(
                    (midpoint.right - midpoint.left - (target - source).abs() * 0.75).abs() < 0.001
                );
                assert!(!motion.running(Duration::from_millis(320)));
            }
        }
    }
    #[test]
    fn retargeting_preserves_geometry_even_on_rapid_reversal() {
        let mut motion = Motion::new(26.0, Duration::from_millis(320));
        let mut seed = 42_u32;
        for milliseconds in (0..10000).step_by(17) {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let now = Duration::from_millis(milliseconds);
            let before = motion.sample(now);
            motion.retarget(26.0 + (seed % 32) as f32 * 20.0, now);
            assert_eq!(motion.sample(now), before);
            let after = motion.sample(now + Duration::from_millis(16));
            assert!(after.left >= 26.0);
            assert!(after.left <= after.right);
            assert!(after.right <= 646.0);
        }
        assert!(!motion.running(Duration::from_secs(20)));
    }
    #[test]
    fn rendered_pixels_are_premultiplied_at_every_supported_size_and_scale() {
        let mut configuration = configuration();
        for slots in 1..=32 {
            configuration.slots = slots;
            for scale in 1..=4 {
                let mut canvas = vec![
                    0;
                    (configuration.width() * configuration.height * scale * scale * 4)
                        as usize
                ];
                let snapshot = Snapshot {
                    monitor: "DP-2".into(),
                    active: slots as i32,
                    occupied: [true; 32],
                    urgent: [false; 32],
                };
                let mut motion = Motion::new(26.0, Duration::from_millis(320));
                motion.retarget(
                    configuration.active_center(slots as i32).unwrap(),
                    Duration::ZERO,
                );
                for milliseconds in [0, 160, 320] {
                    render(
                        &mut canvas,
                        scale,
                        Some(motion.sample(Duration::from_millis(milliseconds))),
                        &configuration,
                        &snapshot,
                    );
                    for pixel in canvas.as_chunks::<4>().0 {
                        let argb = u32::from_ne_bytes(*pixel);
                        for shift in [0, 8, 16] {
                            assert!((argb >> shift) & 255 <= argb >> 24);
                        }
                    }
                    assert_eq!(u32::from_ne_bytes(canvas[..4].try_into().unwrap()), 0);
                }
            }
        }
    }
    #[test]
    fn rendered_capsule_moves_through_intermediate_pixels_and_settles() {
        let configuration = configuration();
        let snapshot = Snapshot {
            monitor: "DP-2".into(),
            active: 5,
            occupied: [true; 32],
            urgent: [false; 32],
        };
        let mut canvas = vec![0; (configuration.width() * configuration.height * 4) as usize];
        let mut motion = Motion::new(26.0, Duration::from_millis(320));
        motion.retarget(106.0, Duration::ZERO);
        let active = 0xffe0e0e0;
        let pixel = |canvas: &[u8], x: u32| {
            let offset = ((configuration.height / 2 * configuration.width() + x) * 4) as usize;
            u32::from_ne_bytes(canvas[offset..offset + 4].try_into().unwrap())
        };
        render(
            &mut canvas,
            1,
            Some(motion.sample(Duration::ZERO)),
            &configuration,
            &snapshot,
        );
        assert_eq!(pixel(&canvas, 26), active);
        assert_ne!(pixel(&canvas, 106), active);
        render(
            &mut canvas,
            1,
            Some(motion.sample(Duration::from_millis(160))),
            &configuration,
            &snapshot,
        );
        assert_eq!(pixel(&canvas, 66), active);
        render(
            &mut canvas,
            1,
            Some(motion.sample(Duration::from_millis(320))),
            &configuration,
            &snapshot,
        );
        assert_ne!(pixel(&canvas, 26), active);
        assert_ne!(pixel(&canvas, 66), active);
        assert_eq!(pixel(&canvas, 106), active);
    }

    #[test]
    fn surrounding_pixels_and_gaps_are_transparent() {
        let configuration = configuration();
        let snapshot = Snapshot {
            monitor: "DP-2".into(),
            active: 1,
            occupied: [true; 32],
            urgent: [false; 32],
        };
        let capsule = Capsule {
            left: 26.0,
            right: 26.0,
        };
        // Reused buffers must be cleared, not just left untouched outside the dots.
        let mut canvas = vec![255; (configuration.width() * configuration.height * 4) as usize];
        render(&mut canvas, 1, Some(capsule), &configuration, &snapshot);
        for (index, pixel) in canvas.as_chunks::<4>().0.iter().enumerate() {
            let x = index as u32 % configuration.width();
            let y = index as u32 / configuration.width();
            if (y as f32 + 0.5 - configuration.height as f32 * 0.5).abs() > 5.5 {
                assert_eq!(*pixel, [0; 4]);
            }
            if x == 36 {
                assert_eq!(*pixel, [0; 4]);
            }
        }
        let center = ((configuration.height / 2 * configuration.width() + 26) * 4) as usize;
        assert_eq!(canvas[center + 3], 255);
    }

    #[test]
    fn rejects_invalid_configuration() {
        let mut configuration = configuration();
        configuration.validate().unwrap();
        assert_eq!(configuration.active_center(0), None);
        assert_eq!(configuration.active_center(6), None);
        configuration.slots = 0;
        assert!(configuration.validate().is_err());
        for color in ["", "ffffff", "#12345", "#gggggg", "#éabcd"] {
            assert!(parse_color(color).is_err());
        }
    }
}
