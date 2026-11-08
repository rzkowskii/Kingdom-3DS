//! Explicit development-mode controls for rendering diagnostics.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderDiagnostics {
    pub painter: bool,
    pub reflection: u8,
}

impl RenderDiagnostics {
    pub fn update(&mut self, enabled: bool, x_pressed: bool, y_pressed: bool, select_held: bool) {
        if !enabled {
            // Leaving development mode restores the normal renderer too.
            *self = Self::default();
            return;
        }
        if x_pressed && !select_held {
            self.painter = !self.painter;
        }
        if y_pressed && !select_held {
            self.reflection = (self.reflection + 1) % 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_buttons_cannot_change_rendering_and_leaving_debug_restores_it() {
        let mut mode = RenderDiagnostics::default();
        mode.update(false, true, true, false);
        assert_eq!(mode, RenderDiagnostics::default());
        mode.update(true, true, true, false);
        assert_eq!(
            mode,
            RenderDiagnostics {
                painter: true,
                reflection: 1
            }
        );
        mode.update(true, true, true, true);
        assert_eq!(
            mode,
            RenderDiagnostics {
                painter: true,
                reflection: 1
            }
        );
        for _ in 0..3 {
            mode.update(true, false, true, false);
        }
        assert_eq!(mode.reflection, 0);
        mode.update(false, false, false, false);
        assert_eq!(mode, RenderDiagnostics::default());
    }
}
