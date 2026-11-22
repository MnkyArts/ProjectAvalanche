use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

pub const NIGHT: Color32 = Color32::from_rgb(0x02, 0x1A, 0x27);
pub const PANEL: Color32 = Color32::from_rgba_unmultiplied_const(0x03, 0x24, 0x33, 0xCC);
pub const PANEL_DEEP: Color32 = Color32::from_rgba_unmultiplied_const(0x02, 0x1C, 0x29, 0xE2);
pub const TAB: [Color32; 2] = [Color32::from_rgb(0x05, 0x45, 0x52), Color32::from_rgb(0x08, 0x5E, 0x6E)];
pub const CYAN: Color32 = Color32::from_rgb(0x5F, 0xE9, 0xFF);
pub const CYAN_SOFT: Color32 = Color32::from_rgb(0x8F, 0xEE, 0xFB);
pub const LINE: Color32 = Color32::from_rgba_unmultiplied_const(0x26, 0x76, 0x98, 0xB4);
pub const LINE_FAINT: Color32 = Color32::from_rgba_unmultiplied_const(0x22, 0x5B, 0x71, 0x78);
pub const TEXT: Color32 = Color32::from_rgb(0xE4, 0xFB, 0xFF);
pub const TEXT_SOFT: Color32 = Color32::from_rgb(0x9A, 0xCA, 0xD8);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x5F, 0x9A, 0xAE);
pub const GOOD: Color32 = Color32::from_rgb(0x3C, 0xE8, 0x9A);
pub const WARN: Color32 = Color32::from_rgb(0xF4, 0xB9, 0x4A);
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0x73, 0x62);
pub const W_REST: Color32 = Color32::from_rgb(0x07, 0x2C, 0x3A);
pub const W_HOVER: Color32 = Color32::from_rgb(0x0A, 0x40, 0x50);
pub const W_ACTIVE: Color32 = Color32::from_rgb(0x0C, 0x58, 0x6A);

pub mod size {
    pub const HERO_TITLE: f32 = 30.0;
    pub const TITLE: f32 = 24.0;
    pub const TILE_TITLE: f32 = 20.0;
    pub const HERO_TEXT: f32 = 18.0;
    pub const TEXT: f32 = 16.0;
    pub const WIDGET: f32 = 15.0;
    pub const MENU_TEXT: f32 = 15.0;
    pub const SMALL: f32 = 14.0;
    pub const CAPS: f32 = 13.0;
    pub const CHIP: f32 = 11.0;
}

pub const CAP_HEIGHT: f32 = 0.688;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Text,
    Title,
    Bold,
}

impl Face {
    pub fn family(self) -> FontFamily {
        match self {
            Self::Text => FontFamily::Proportional,
            Self::Title => FontFamily::Name("title".into()),
            Self::Bold => FontFamily::Name("bold".into()),
        }
    }

    pub fn id(self, size: f32) -> FontId {
        FontId::new(size, self.family())
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (i, s) in ssx_content::font::shipped::SHIPPED.iter().enumerate() {
        match ssx_content::font::shipped::read(i) {
            Ok(bytes) => {
                fonts.font_data.insert(s.id.into(), FontData::from_owned(bytes).into());
                fallback.push(s.id.to_string());
            }
            Err(e) => ssx_core::eprintln!("fonts: {} unavailable: {e}", s.id),
        }
    }
    for (name, family, bytes) in [
        ("saira-medium", Face::Text, &include_bytes!("../../assets/fonts/SairaCondensed-Medium.ttf")[..]),
        ("saira-semibold", Face::Title, &include_bytes!("../../assets/fonts/SairaCondensed-SemiBold.ttf")[..]),
        ("saira-bold", Face::Bold, &include_bytes!("../../assets/fonts/SairaCondensed-Bold.ttf")[..]),
    ] {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
        let mut list = vec![name.to_string()];
        list.extend(fallback.iter().cloned());
        fonts.families.insert(family.family(), list);
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    ctx.all_styles_mut(style);
}

fn style(style: &mut egui::Style) {
    use size::{SMALL, TILE_TITLE, WIDGET};
    style.text_styles = [
        (TextStyle::Heading, Face::Title.id(TILE_TITLE)),
        (TextStyle::Body, Face::Text.id(WIDGET)),
        (TextStyle::Button, Face::Text.id(WIDGET)),
        (TextStyle::Small, Face::Text.id(SMALL)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    let sp = &mut style.spacing;
    sp.item_spacing = egui::vec2(8.0, 8.0);
    sp.button_padding = egui::vec2(12.0, 2.0);
    sp.interact_size = egui::vec2(40.0, 28.0);
    sp.icon_width = 18.0;
    sp.icon_width_inner = 10.0;
    sp.icon_spacing = 8.0;
    sp.slider_width = 160.0;
    sp.slider_rail_height = 4.0;
    sp.combo_width = 220.0;
    sp.menu_margin = egui::Margin::same(4);
    sp.scroll = egui::style::ScrollStyle {
        bar_width: 6.0,
        foreground_color: true,
        dormant_background_opacity: 0.25,
        dormant_handle_opacity: 0.45,
        ..egui::style::ScrollStyle::solid()
    };

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT_SOFT);
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = Color32::from_rgb(0x04, 0x1C, 0x28);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.extreme_bg_color = Color32::from_rgb(0x02, 0x10, 0x18);
    v.faint_bg_color = Color32::from_rgba_premultiplied(0x06, 0x22, 0x2E, 0x80);
    v.code_bg_color = v.extreme_bg_color;
    v.hyperlink_color = CYAN;
    v.selection.bg_fill = Color32::from_rgb(0x0B, 0x6E, 0x80);
    v.selection.stroke = Stroke::new(1.0, CYAN);
    v.slider_trailing_fill = true;
    v.popup_shadow = egui::Shadow::NONE;
    v.window_shadow = egui::Shadow::NONE;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERROR;
    let fills = [
        (&mut v.widgets.noninteractive, Color32::from_rgb(0x04, 0x1C, 0x28), LINE_FAINT, TEXT_SOFT),
        (&mut v.widgets.inactive, W_REST, LINE, TEXT_SOFT),
        (&mut v.widgets.hovered, W_HOVER, CYAN, TEXT),
        (&mut v.widgets.active, W_ACTIVE, CYAN, TEXT),
        (&mut v.widgets.open, Color32::from_rgb(0x08, 0x38, 0x48), CYAN, TEXT),
    ];
    for (w, fill, line, text) in fills {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, line);
        w.fg_stroke = Stroke::new(1.5, text);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }
}
