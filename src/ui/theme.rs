use gpui::{App, Global, Hsla, Pixels, SharedString, px, rgb};
use gpui_component::Theme;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(super) struct ThemeColors {
    pub(super) surface_background: Hsla,
    pub(super) surface_foreground: Hsla,
    pub(super) panel_background: Hsla,
    pub(super) input_background: Hsla,
    pub(super) panel_border: Hsla,
    pub(super) panel_active_background: Hsla,
    pub(super) panel_active_border: Hsla,
    pub(super) primary: Hsla,
    pub(super) primary_dark: Hsla,
    pub(super) muted_foreground: Hsla,
    pub(super) accent_foreground: Hsla,
    pub(super) success_foreground: Hsla,
    pub(super) error_foreground: Hsla,
    pub(super) warning_foreground: Hsla,
    pub(super) progress_foreground: Hsla,
    pub(super) drop_invalid_border: Hsla,
    pub(super) drop_invalid_background: Hsla,
    pub(super) track_purple: Hsla,
    pub(super) track_blue: Hsla,
    pub(super) track_green: Hsla,
    pub(super) track_red: Hsla,
    pub(super) track_orange: Hsla,
    pub(super) track_cyan: Hsla,
    pub(super) glow_primary: Hsla,
    pub(super) glow_purple: Hsla,
    pub(super) glow_blue: Hsla,
    pub(super) glow_green: Hsla,
    pub(super) glow_red: Hsla,
    pub(super) glow_orange: Hsla,
    pub(super) glow_cyan: Hsla,
}

impl ThemeColors {
    #[inline]
    pub(super) fn selectable_panel_border(self, selected: bool) -> Hsla {
        if selected {
            self.panel_active_border
        } else {
            self.panel_border
        }
    }

    #[inline]
    pub(super) fn selectable_panel_background(self, selected: bool) -> Hsla {
        if selected {
            self.panel_active_background
        } else {
            self.panel_background
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ThemeTypography {
    pub(super) font_family: SharedString,
    pub(super) mono_font_family: SharedString,
    pub(super) font_size: Pixels,
    pub(super) mono_font_size: Pixels,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ThemeSpacing {
    pub(super) window_padding: Pixels,
    pub(super) section_gap: Pixels,
    pub(super) panel_padding: Pixels,
    pub(super) panel_compact_padding: Pixels,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ThemeRadius {
    pub(super) control: Pixels,
    pub(super) panel: Pixels,
}

#[derive(Debug, Clone)]
pub(super) struct SonantTheme {
    pub(super) colors: ThemeColors,
    pub(super) typography: ThemeTypography,
    pub(super) spacing: ThemeSpacing,
    pub(super) radius: ThemeRadius,
}

impl Default for SonantTheme {
    fn default() -> Self {
        Self {
            colors: ThemeColors {
                surface_background: rgb(0x101322).into(),
                surface_foreground: rgb(0xf9fafb).into(),
                panel_background: rgb(0x161b2e).into(),
                input_background: rgb(0x1d233b).into(),
                panel_border: rgb(0x2a3254).into(),
                panel_active_background: rgb(0x1d233b).into(),
                panel_active_border: rgb(0x1032e2).into(),
                primary: rgb(0x1032e2).into(),
                primary_dark: rgb(0x0b24a8).into(),
                muted_foreground: rgb(0x94a3b8).into(),
                accent_foreground: rgb(0x93c5fd).into(),
                success_foreground: rgb(0x86efac).into(),
                error_foreground: rgb(0xfca5a5).into(),
                warning_foreground: rgb(0xfcd34d).into(),
                progress_foreground: rgb(0xfbbf24).into(),
                drop_invalid_border: rgb(0xfda4af).into(),
                drop_invalid_background: rgb(0x3f1d2e).into(),
                track_purple: rgb(0xa855f7).into(),
                track_blue: rgb(0x3b82f6).into(),
                track_green: rgb(0x22c55e).into(),
                track_red: rgb(0xef4444).into(),
                track_orange: rgb(0xf97316).into(),
                track_cyan: rgb(0x06b6d4).into(),
                glow_primary: rgb(0x1032e2).into(),
                glow_purple: rgb(0xa855f7).into(),
                glow_blue: rgb(0x3b82f6).into(),
                glow_green: rgb(0x22c55e).into(),
                glow_red: rgb(0xef4444).into(),
                glow_orange: rgb(0xf97316).into(),
                glow_cyan: rgb(0x06b6d4).into(),
            },
            typography: ThemeTypography {
                font_family: ".SystemUIFont".into(),
                mono_font_family: if cfg!(target_os = "macos") {
                    "Menlo".into()
                } else if cfg!(target_os = "windows") {
                    "Consolas".into()
                } else {
                    "DejaVu Sans Mono".into()
                },
                font_size: px(16.0),
                mono_font_size: px(13.0),
            },
            spacing: ThemeSpacing {
                window_padding: px(16.0),
                section_gap: px(12.0),
                panel_padding: px(12.0),
                panel_compact_padding: px(8.0),
            },
            radius: ThemeRadius {
                control: px(6.0),
                panel: px(10.0),
            },
        }
    }
}

impl Global for SonantTheme {}

pub(super) fn apply_default_theme(cx: &mut App) {
    apply_theme(SonantTheme::default(), cx);
}

pub(super) fn apply_theme(theme: SonantTheme, cx: &mut App) {
    cx.set_global(theme.clone());
    apply_to_gpui_component_theme(&theme, cx);
}

fn apply_to_gpui_component_theme(theme: &SonantTheme, cx: &mut App) {
    let component_theme = Theme::global_mut(cx);

    component_theme.font_family = theme.typography.font_family.clone();
    component_theme.font_size = theme.typography.font_size;
    component_theme.mono_font_family = theme.typography.mono_font_family.clone();
    component_theme.mono_font_size = theme.typography.mono_font_size;

    component_theme.radius = theme.radius.control;
    component_theme.radius_lg = theme.radius.panel;

    component_theme.background = theme.colors.surface_background;
    component_theme.foreground = theme.colors.surface_foreground;
    component_theme.border = theme.colors.panel_border;
    component_theme.input = theme.colors.panel_border;

    component_theme.primary = theme.colors.primary;
    component_theme.primary_hover = theme.colors.primary_dark;
    component_theme.primary_active = theme.colors.primary;
    component_theme.primary_foreground = theme.colors.surface_foreground;

    component_theme.secondary = theme.colors.panel_background;
    component_theme.secondary_hover = theme.colors.panel_active_background;
    component_theme.secondary_active = theme.colors.panel_active_background;
    component_theme.secondary_foreground = theme.colors.surface_foreground;

    component_theme.danger = theme.colors.error_foreground;
    component_theme.danger_hover = theme.colors.error_foreground;
    component_theme.danger_active = theme.colors.error_foreground;
    component_theme.danger_foreground = theme.colors.surface_background;

    component_theme.success = theme.colors.success_foreground;
    component_theme.success_hover = theme.colors.success_foreground;
    component_theme.success_active = theme.colors.success_foreground;
    component_theme.success_foreground = theme.colors.surface_background;

    component_theme.warning = theme.colors.warning_foreground;
    component_theme.warning_hover = theme.colors.warning_foreground;
    component_theme.warning_active = theme.colors.warning_foreground;
    component_theme.warning_foreground = theme.colors.surface_background;

    component_theme.info = theme.colors.accent_foreground;
    component_theme.info_hover = theme.colors.accent_foreground;
    component_theme.info_active = theme.colors.accent_foreground;
    component_theme.info_foreground = theme.colors.surface_background;

    component_theme.muted_foreground = theme.colors.muted_foreground;
    component_theme.ring = theme.colors.primary;

    component_theme.popover = theme.colors.panel_background;
    component_theme.popover_foreground = theme.colors.surface_foreground;

    component_theme.list = theme.colors.panel_background;
    component_theme.list_hover = theme.colors.panel_active_background;
    component_theme.list_active = theme.colors.panel_active_background;
    component_theme.list_active_border = theme.colors.panel_active_border;
}
