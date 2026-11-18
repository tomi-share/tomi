//! Pure geometry for placing the popover next to the tray icon.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

pub const MARGIN: i32 = 8;

/// Top-left corner for a window of `size`, in physical pixels.
///
/// With an anchor (the tray icon rectangle) the window is centred horizontally on it and
/// placed above it when the icon is in the lower half of the work area (taskbar at the
/// bottom), below it otherwise. Without an anchor it goes to the bottom-right corner.
/// The result is always clamped inside the work area.
pub fn popover_position(anchor: Option<Area>, size: (i32, i32), work: Area) -> (i32, i32) {
    let (w, h) = size;
    let (mut x, mut y) = match anchor {
        Some(a) => {
            let x = a.x + a.w / 2 - w / 2;
            let icon_in_lower_half = a.y + a.h / 2 > work.y + work.h / 2;
            let y = if icon_in_lower_half {
                a.y - h - MARGIN
            } else {
                a.bottom() + MARGIN
            };
            (x, y)
        }
        None => (work.right() - w - MARGIN, work.bottom() - h - MARGIN),
    };
    x = x.min(work.right() - w - MARGIN).max(work.x + MARGIN);
    y = y.min(work.bottom() - h - MARGIN).max(work.y + MARGIN);
    (x, y)
}

/// Clicking the tray icon while the popover is open first moves focus away from it (it
/// hides itself), then the click arrives. Without this guard the click would reopen
/// the popover that the user meant to close.
pub const REOPEN_GUARD: Duration = Duration::from_millis(300);

pub fn ignore_tray_click(hidden_by_blur_at: Option<Instant>, now: Instant) -> bool {
    hidden_by_blur_at.is_some_and(|t| now.saturating_duration_since(t) < REOPEN_GUARD)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Area = Area {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };
    const SIZE: (i32, i32) = (400, 600);

    #[test]
    fn tray_at_the_bottom_places_the_popover_above() {
        let tray = Area {
            x: 1700,
            y: 1048,
            w: 24,
            h: 32,
        };
        let (x, y) = popover_position(Some(tray), SIZE, WORK);
        assert_eq!(x, 1700 + 12 - 200);
        // Above the icon, and kept MARGIN away from the taskbar (the work area ends at
        // 1040, the icon starts at 1048: the clamp wins over "icon.y - h - MARGIN").
        assert_eq!(y, WORK.bottom() - 600 - MARGIN);
        assert!(y + 600 <= tray.y);
    }

    #[test]
    fn tray_at_the_top_places_the_popover_below() {
        let work = Area {
            x: 0,
            y: 40,
            w: 1920,
            h: 1040,
        };
        let tray = Area {
            x: 1000,
            y: 0,
            w: 24,
            h: 32,
        };
        let (_, y) = popover_position(Some(tray), SIZE, work);
        // Below the icon (which ends at 32), kept MARGIN away from the work area edge (40).
        assert_eq!(y, work.y + MARGIN);
        assert!(y >= tray.bottom());
    }

    #[test]
    fn clamped_inside_the_work_area() {
        let tray = Area {
            x: 1900,
            y: 1048,
            w: 24,
            h: 32,
        };
        let (x, _) = popover_position(Some(tray), SIZE, WORK);
        assert_eq!(x, WORK.right() - 400 - MARGIN);
        let tray = Area {
            x: 0,
            y: 1048,
            w: 24,
            h: 32,
        };
        let (x, _) = popover_position(Some(tray), SIZE, WORK);
        assert_eq!(x, MARGIN);
    }

    #[test]
    fn without_anchor_goes_bottom_right() {
        let (x, y) = popover_position(None, SIZE, WORK);
        assert_eq!((x, y), (1920 - 400 - MARGIN, 1040 - 600 - MARGIN));
    }

    #[test]
    fn works_on_a_secondary_monitor_with_negative_origin() {
        let work = Area {
            x: -1920,
            y: 0,
            w: 1920,
            h: 1040,
        };
        let tray = Area {
            x: -100,
            y: 1048,
            w: 24,
            h: 32,
        };
        let (x, y) = popover_position(Some(tray), SIZE, work);
        assert!(x >= work.x && x + 400 <= work.right());
        assert!(y >= work.y && y + 600 <= work.bottom());
    }

    #[test]
    fn window_taller_than_the_area_is_pinned_to_the_top_margin() {
        let work = Area {
            x: 0,
            y: 0,
            w: 1920,
            h: 500,
        };
        let (_, y) = popover_position(None, SIZE, work);
        assert_eq!(y, MARGIN);
    }

    #[test]
    fn tray_click_right_after_a_blur_hide_is_ignored() {
        let t0 = Instant::now();
        assert!(ignore_tray_click(Some(t0), t0 + Duration::from_millis(100)));
        assert!(!ignore_tray_click(
            Some(t0),
            t0 + Duration::from_millis(500)
        ));
        assert!(!ignore_tray_click(None, t0));
    }
}
