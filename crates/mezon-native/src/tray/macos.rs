use std::cell::Cell;
use std::cell::RefCell;

use anyhow::{Context, Result};
use dispatch2::DispatchQueue;
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAccessibilityHighContrastDarkAqua,
    NSAppearanceNameDarkAqua, NSAppearanceNameVibrantDark, NSBackingStoreType, NSBezelStyle,
    NSButton, NSColor, NSEvent, NSEventMask, NSFont, NSFontWeightSemibold, NSImage, NSImageScaling,
    NSImageSymbolConfiguration, NSImageSymbolScale, NSImageView, NSPanel, NSScreen,
    NSStatusWindowLevel, NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions,
    NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_graphics::{CGDisplayPixelsHigh, CGMainDisplayID};
use objc2_foundation::{
    MainThreadMarker, NSObjectProtocol, NSPoint, NSPointInRect, NSRect, NSSize, NSString,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use super::{
    TRAY_ICON_BYTES, TrayUiAction, TrayVoiceState, draw_call_indicator, load_base_icon_rgba,
};

mod action_tag {
    pub const SHOW: isize = 1;
    pub const UPDATE: isize = 2;
    pub const QUIT: isize = 3;
    pub const MIC: isize = 4;
    pub const CAMERA: isize = 5;
    pub const SCREEN: isize = 6;
    pub const LEAVE: isize = 7;

    pub fn dismisses_panel(tag: isize) -> bool {
        matches!(tag, SHOW | UPDATE | QUIT)
    }
}

const DEFAULT_LOCALE: &str = "en";
const DEFAULT_SCREEN_SCALE: f64 = 2.0;

const PANEL_WIDTH: f64 = 300.0;
const PANEL_PADDING: f64 = 12.0;
const PANEL_ABOVE_TRAY_GAP: f64 = 6.0;
const PANEL_WINDOW_LEVEL_OFFSET: isize = 1;
const POPOVER_BORDER_WIDTH: f64 = 1.0;

const ROW_HEIGHT: f64 = 30.0;
const ROW_GAP: f64 = 1.0;
const ROW_CONTENT_INSET: f64 = 2.0;
const MENU_ROW_COUNT: f64 = 3.0;
const MENU_ROW_HOVER_RADIUS: f64 = 8.0;

const VOICE_CARD_PADDING: f64 = 10.0;
const VOICE_CARD_CORNER_RADIUS: f64 = 10.0;
const VOICE_CARD_GAP: f64 = 10.0;
const VOICE_CARD_SECTION_GAP: f64 = 8.0;
const VOICE_CONTROL_SIZE: f64 = 32.0;
const VOICE_CONTROL_GAP: f64 = 10.0;
const VOICE_CONTROL_COUNT: f64 = 4.0;
const VOICE_CONTROL_ICON_SIZE: f64 = 14.0;

const SECTION_LABEL_HEIGHT: f64 = 14.0;
const SECTION_LABEL_GAP: f64 = 4.0;
const STATUS_LINE_HEIGHT: f64 = 18.0;
const STATUS_LINE_GAP: f64 = 2.0;
const SUBTITLE_LINE_HEIGHT: f64 = 16.0;
const SUBTITLE_LINE_GAP: f64 = 2.0;

const VOICE_CARD_HEIGHT: f64 = VOICE_CARD_PADDING * 2.0
    + SECTION_LABEL_HEIGHT
    + SECTION_LABEL_GAP
    + STATUS_LINE_HEIGHT
    + STATUS_LINE_GAP
    + SUBTITLE_LINE_HEIGHT
    + SUBTITLE_LINE_GAP
    + VOICE_CARD_SECTION_GAP
    + VOICE_CONTROL_SIZE;

const IDLE_ICON_SIZE: f64 = 22.0;
const IDLE_ICON_MESSAGE_GAP: f64 = 6.0;
const IDLE_MESSAGE_HEIGHT: f64 = 18.0;

const CORNER_RADIUS: f64 = 14.0;
const ICON_BADGE_SIZE: f64 = 22.0;
const ICON_BADGE_INSET: f64 = 4.0;
const ICON_TEXT_GAP: f64 = 10.0;

const MENU_FONT_SIZE: f64 = 13.0;
const SECTION_FONT_SIZE: f64 = 11.0;
const STATUS_FONT_SIZE: f64 = 13.0;
const SUBTITLE_FONT_SIZE: f64 = 12.0;
const IDLE_MESSAGE_FONT_SIZE: f64 = 13.0;

struct TrayIconFrame {
    x: f64,
    width: f64,
    height: f64,
    top: f64,
}

fn panel_action_sel() -> objc2::runtime::Sel {
    sel!(performPanelAction:)
}

thread_local! {
    static TRAY_PANEL: RefCell<Option<TrayPanel>> = const { RefCell::new(None) };
    static TRAY_ICON: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

struct TrayPanel {
    voice_state: TrayVoiceState,
    locale: String,
    shown: bool,
    panel: Retained<NSPanel>,
    action_target: Retained<PanelActionTarget>,
    dismiss_monitors: Vec<Retained<AnyObject>>,
    mtm: MainThreadMarker,
}

struct PanelActionTargetIvars {
    _marker: Cell<()>,
}

struct MenuRowViewIvars {
    is_dark: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "MezonTrayMenuRowView"]
    #[ivars = MenuRowViewIvars]
    struct MenuRowView;

    impl MenuRowView {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            set_menu_row_hover(self, self.ivars().is_dark.get(), true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            set_menu_row_hover(self, self.ivars().is_dark.get(), false);
        }
    }
);

define_class!(
    #[unsafe(super(objc2::runtime::NSObject))]
    #[name = "MezonTrayPanelActionTarget"]
    #[ivars = PanelActionTargetIvars]
    struct PanelActionTarget;

    impl PanelActionTarget {
        #[unsafe(method(performPanelAction:))]
        fn perform_panel_action(&self, sender: &NSButton) {
            let tag = sender.tag();
            let action = match tag {
                action_tag::SHOW => TrayUiAction::Show,
                action_tag::UPDATE => TrayUiAction::Update,
                action_tag::QUIT => TrayUiAction::Quit,
                action_tag::MIC => TrayUiAction::Voice(super::TrayVoiceAction::ToggleMic),
                action_tag::CAMERA => TrayUiAction::Voice(super::TrayVoiceAction::ToggleCamera),
                action_tag::SCREEN => TrayUiAction::Voice(super::TrayVoiceAction::ToggleScreen),
                action_tag::LEAVE => TrayUiAction::Voice(super::TrayVoiceAction::Leave),
                _ => return,
            };
            if let Some(tx) = ACTION_TX.get() {
                let _ = tx.send(action);
            }
            if action_tag::dismisses_panel(tag) {
                with_panel(|panel| panel.hide());
            }
        }
    }
);

static ACTION_TX: std::sync::OnceLock<crossbeam_channel::Sender<TrayUiAction>> =
    std::sync::OnceLock::new();

fn with_panel<R>(f: impl FnOnce(&mut TrayPanel) -> R) -> Option<R> {
    TRAY_PANEL.with(|cell| cell.borrow_mut().as_mut().map(f))
}

pub fn register_tray_icon(icon: TrayIcon) {
    TRAY_ICON.with(|cell| *cell.borrow_mut() = Some(icon));
}

pub fn toggle_panel() {
    DispatchQueue::main().exec_async(|| {
        let icon = TRAY_ICON.with(|cell| cell.borrow().clone());
        let Some(icon) = icon else {
            return;
        };
        with_panel(|panel| {
            if panel.shown {
                panel.hide();
            } else {
                panel.show_with_icon(&icon);
            }
        });
    });
}

pub struct MacosTray {
    icon: TrayIcon,
    base_icon: RgbaImage,
}

impl MacosTray {
    pub fn new(
        voice_state: TrayVoiceState,
        locale: String,
        action_tx: crossbeam_channel::Sender<TrayUiAction>,
    ) -> Result<Self> {
        let _ = ACTION_TX.set(action_tx);

        let mtm = MainThreadMarker::new().context("tray must be created on the main thread")?;
        let base_icon = load_base_icon_rgba(TRAY_ICON_BYTES)?;
        let icon = build_tray_icon(&base_icon, false)?;

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Mezon")
            .with_icon(icon.clone())
            .with_icon_as_template(false)
            .with_menu_on_left_click(false)
            .build()
            .context("failed to build macOS tray icon")?;

        let this = mtm.alloc().set_ivars(PanelActionTargetIvars {
            _marker: Cell::new(()),
        });
        let action_target: Retained<PanelActionTarget> = unsafe { msg_send![super(this), init] };

        let panel = create_panel(mtm, &action_target)?;
        TRAY_PANEL.with(|cell| {
            *cell.borrow_mut() = Some(TrayPanel {
                voice_state,
                locale,
                shown: false,
                panel,
                action_target,
                dismiss_monitors: Vec::new(),
                mtm,
            });
        });
        register_tray_icon(tray_icon.clone());

        Ok(Self {
            icon: tray_icon,
            base_icon,
        })
    }

    pub fn update_voice_state(&mut self, state: TrayVoiceState) {
        if let Ok(icon) = build_tray_icon(&self.base_icon, state.in_call) {
            let _ = self.icon.set_icon_with_as_template(Some(icon), false);
        }
        register_tray_icon(self.icon.clone());

        let tray_icon = self.icon.clone();
        with_panel(|panel| {
            panel.voice_state = state;
            if panel.shown {
                panel.refresh_content();
                panel.position_panel(&tray_icon);
            }
        });
    }

    pub fn update_locale(&mut self, locale: String) {
        with_panel(|panel| {
            panel.locale = locale;
            if panel.shown {
                panel.refresh_content();
            }
        });
    }
}

impl TrayPanel {
    fn show_with_icon(&mut self, tray_icon: &TrayIcon) {
        self.refresh_content();
        self.position_panel(tray_icon);
        self.panel.orderFrontRegardless();
        self.shown = true;
        self.install_dismiss_monitor();
    }

    fn hide(&mut self) {
        self.panel.orderOut(None);
        self.shown = false;
        self.remove_dismiss_monitor();
    }

    fn refresh_content(&self) {
        let appearance = self.panel.effectiveAppearance();
        let content = build_panel_content(
            self.mtm,
            &self.action_target,
            &self.voice_state,
            &self.locale,
            &appearance,
        );
        let height = panel_height(&self.voice_state);
        self.panel.setContentSize(NSSize::new(PANEL_WIDTH, height));
        self.panel.setContentView(Some(&content));
    }

    fn position_panel(&mut self, tray_icon: &TrayIcon) {
        let Some(frame) = tray_icon_frame(tray_icon) else {
            tracing::warn!("Tray icon rect unavailable; using default panel position");
            return;
        };

        let panel_height = panel_height(&self.voice_state);
        let panel_x = frame.x + frame.width / 2.0 - PANEL_WIDTH / 2.0;
        let panel_top = frame.top + frame.height + PANEL_ABOVE_TRAY_GAP;
        let panel_y = screen_height_points() - panel_top - panel_height;

        self.panel.setFrameOrigin(NSPoint::new(panel_x, panel_y));
    }

    fn install_dismiss_monitor(&mut self) {
        self.remove_dismiss_monitor();
        let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown;

        let global_block = block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| {
            dismiss_panel_for_outside_click(event);
        });
        if let Some(monitor) =
            NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block)
        {
            self.dismiss_monitors.push(monitor);
        }

        let local_block =
            block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
                dismiss_panel_for_outside_click(event);
                event.as_ptr()
            });
        if let Some(monitor) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) }
        {
            self.dismiss_monitors.push(monitor);
        }
    }

    fn remove_dismiss_monitor(&mut self) {
        for monitor in self.dismiss_monitors.drain(..) {
            unsafe {
                NSEvent::removeMonitor(&monitor);
            }
        }
    }
}

fn dismiss_panel_for_outside_click(event: std::ptr::NonNull<NSEvent>) {
    TRAY_PANEL.with(|cell| {
        let mut panel_ref = cell.borrow_mut();
        let Some(panel) = panel_ref.as_mut() else {
            return;
        };
        if !panel.shown {
            return;
        }
        if click_is_on_tray_icon() {
            return;
        }
        let event = unsafe { event.as_ref() };
        if click_is_inside_panel(panel, event) {
            return;
        }
        panel.hide();
    });
}

fn click_is_inside_panel(panel: &TrayPanel, event: &NSEvent) -> bool {
    let mtm = panel.mtm;
    let panel_ptr = Retained::as_ptr(&panel.panel) as *const NSWindow;
    if event
        .window(mtm)
        .is_some_and(|window| Retained::as_ptr(&window) == panel_ptr)
    {
        return true;
    }

    let mouse = NSEvent::mouseLocation();
    NSPointInRect(mouse, panel.panel.frame())
}

fn click_is_on_tray_icon() -> bool {
    TRAY_ICON.with(|cell| {
        let icon_ref = cell.borrow();
        let Some(icon) = icon_ref.as_ref() else {
            return false;
        };
        let Some(frame) = tray_icon_frame(icon) else {
            return false;
        };
        NSPointInRect(NSEvent::mouseLocation(), tray_icon_screen_rect(&frame))
    })
}

fn tray_icon_frame(icon: &TrayIcon) -> Option<TrayIconFrame> {
    let rect = icon.rect()?;
    let scale = main_screen_scale();
    Some(TrayIconFrame {
        x: rect.position.x / scale,
        width: rect.size.width as f64 / scale,
        height: rect.size.height as f64 / scale,
        top: rect.position.y / scale,
    })
}

fn tray_icon_screen_rect(frame: &TrayIconFrame) -> NSRect {
    let bottom = screen_height_points() - frame.top - frame.height;
    NSRect::new(
        NSPoint::new(frame.x, bottom),
        NSSize::new(frame.width, frame.height),
    )
}

fn create_panel(
    mtm: MainThreadMarker,
    action_target: &PanelActionTarget,
) -> Result<Retained<NSPanel>> {
    let initial_height = panel_height(&TrayVoiceState::default());
    let content_rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(PANEL_WIDTH, initial_height),
    );
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        content_rect,
        style,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    panel.setLevel(NSStatusWindowLevel + PANEL_WINDOW_LEVEL_OFFSET);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    let appearance = panel.effectiveAppearance();
    let content = build_panel_content(
        mtm,
        action_target,
        &TrayVoiceState::default(),
        DEFAULT_LOCALE,
        &appearance,
    );
    panel.setContentView(Some(&content));
    Ok(panel)
}

fn configure_rounded_container(container: &NSView, theme: &VoiceTheme) {
    with_view_layer(container, false, |layer| {
        let _: () = unsafe { msg_send![layer, setCornerRadius: CORNER_RADIUS] };
        let _: () = unsafe { msg_send![layer, setBorderWidth: POPOVER_BORDER_WIDTH] };
        let border_cg = theme.popover_border_color().CGColor();
        let _: () = unsafe { msg_send![layer, setBorderColor: &*border_cg] };
    });
}

fn configure_effect_view(effect: &NSVisualEffectView) {
    effect.setMaterial(NSVisualEffectMaterial::Popover);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setEmphasized(false);
    with_view_layer(effect, true, |layer| {
        let _: () = unsafe { msg_send![layer, setCornerRadius: CORNER_RADIUS] };
    });
}

fn with_view_layer(view: &NSView, masks_to_bounds: bool, configure: impl FnOnce(&AnyObject)) {
    view.setWantsLayer(true);
    view.setClipsToBounds(masks_to_bounds);
    if !view.respondsToSelector(sel!(layer)) {
        return;
    }
    let layer: Option<Retained<AnyObject>> = unsafe { msg_send![view, layer] };
    if let Some(layer) = layer {
        let _: () = unsafe { msg_send![&layer, setMasksToBounds: masks_to_bounds] };
        configure(&layer);
    }
}

fn system_symbol(name: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    image.setTemplate(true);
    Some(image)
}

fn symbol_configuration() -> Retained<NSImageSymbolConfiguration> {
    NSImageSymbolConfiguration::configurationWithScale(NSImageSymbolScale::Small)
}

fn voice_control_symbol_configuration(
    icon_color: &NSColor,
) -> Retained<NSImageSymbolConfiguration> {
    let weight = unsafe { NSFontWeightSemibold };
    let size = NSImageSymbolConfiguration::configurationWithPointSize_weight(
        VOICE_CONTROL_ICON_SIZE,
        weight,
    );
    let color = NSImageSymbolConfiguration::configurationWithHierarchicalColor(icon_color);
    size.configurationByApplyingConfiguration(&color)
}

fn idle_voice_symbol_configuration(icon_color: &NSColor) -> Retained<NSImageSymbolConfiguration> {
    let size = NSImageSymbolConfiguration::configurationWithPointSize_weight(IDLE_ICON_SIZE, 0.0);
    let color = NSImageSymbolConfiguration::configurationWithHierarchicalColor(icon_color);
    size.configurationByApplyingConfiguration(&color)
}

fn secondary_text_color() -> Retained<NSColor> {
    NSColor::secondaryLabelColor()
}

fn section_text_color() -> Retained<NSColor> {
    NSColor::tertiaryLabelColor()
}

fn badge_background_color() -> Retained<NSColor> {
    NSColor::quaternaryLabelColor()
}

fn is_dark_appearance(appearance: &NSAppearance) -> bool {
    let name = appearance.name();
    unsafe {
        if name.isEqualToString(NSAppearanceNameDarkAqua)
            || name.isEqualToString(NSAppearanceNameVibrantDark)
            || name.isEqualToString(NSAppearanceNameAccessibilityHighContrastDarkAqua)
        {
            return true;
        }
    }
    name.containsString(&NSString::from_str("Dark"))
}

struct VoiceTheme {
    is_dark: bool,
}

impl VoiceTheme {
    fn from_appearance(appearance: &NSAppearance) -> Self {
        Self {
            is_dark: is_dark_appearance(appearance),
        }
    }

    fn card_background(&self) -> Retained<NSColor> {
        if self.is_dark {
            NSColor::colorWithWhite_alpha(1.0, 0.10)
        } else {
            NSColor::colorWithWhite_alpha(0.0, 0.08)
        }
    }

    fn control_off_background(&self) -> Retained<NSColor> {
        if self.is_dark {
            NSColor::colorWithWhite_alpha(1.0, 0.22)
        } else {
            NSColor::colorWithWhite_alpha(0.0, 0.08)
        }
    }

    fn control_off_icon(&self) -> Retained<NSColor> {
        if self.is_dark {
            NSColor::whiteColor()
        } else {
            NSColor::colorWithWhite_alpha(0.0, 0.42)
        }
    }

    fn status_color(&self) -> Retained<NSColor> {
        if self.is_dark {
            NSColor::systemGreenColor()
        } else {
            NSColor::colorWithRed_green_blue_alpha(0.06, 0.52, 0.16, 1.0)
        }
    }

    fn idle_status_color(&self) -> Retained<NSColor> {
        if self.is_dark {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::colorWithWhite_alpha(0.0, 0.45)
        }
    }

    fn popover_border_color(&self) -> Retained<NSColor> {
        if self.is_dark {
            NSColor::colorWithWhite_alpha(1.0, 0.14)
        } else {
            NSColor::colorWithWhite_alpha(0.0, 0.12)
        }
    }
}

fn menu_row_hover_color(is_dark: bool) -> Retained<NSColor> {
    if is_dark {
        NSColor::colorWithWhite_alpha(1.0, 0.12)
    } else {
        NSColor::colorWithWhite_alpha(0.0, 0.06)
    }
}

fn voice_control_style(
    theme: &VoiceTheme,
    enabled: bool,
    disconnect: bool,
) -> (Retained<NSColor>, Retained<NSColor>) {
    if disconnect {
        (NSColor::systemRedColor(), NSColor::whiteColor())
    } else if enabled {
        (NSColor::systemGreenColor(), NSColor::whiteColor())
    } else {
        (theme.control_off_background(), theme.control_off_icon())
    }
}

fn create_menu_row_view(
    mtm: MainThreadMarker,
    frame: NSRect,
    is_dark: bool,
) -> Retained<MenuRowView> {
    let this = MenuRowView::alloc(mtm).set_ivars(MenuRowViewIvars {
        is_dark: Cell::new(is_dark),
    });
    let row: Retained<MenuRowView> = unsafe { msg_send![super(this), initWithFrame: frame] };
    row.setWantsLayer(true);
    set_menu_row_hover(&row, is_dark, false);
    install_menu_row_tracking(&row);
    row
}

fn install_menu_row_tracking(row: &MenuRowView) {
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            options,
            Some(row),
            None,
        )
    };
    row.addTrackingArea(&area);
}

fn set_menu_row_hover(row: &NSView, is_dark: bool, hovered: bool) {
    with_view_layer(row, false, |layer| {
        let _: () = unsafe { msg_send![layer, setCornerRadius: MENU_ROW_HOVER_RADIUS] };
        let background = if hovered {
            menu_row_hover_color(is_dark).CGColor()
        } else {
            NSColor::clearColor().CGColor()
        };
        let _: () = unsafe { msg_send![layer, setBackgroundColor: &*background] };
    });
}

fn apply_layer_style(view: &NSView, background: &NSColor, corner_radius: f64) {
    with_view_layer(view, true, |layer| {
        let _: () = unsafe { msg_send![layer, setCornerRadius: corner_radius] };
        let _: () = unsafe { msg_send![layer, setBorderWidth: 0.0] };
        let clear_cg = NSColor::clearColor().CGColor();
        let _: () = unsafe { msg_send![layer, setBorderColor: &*clear_cg] };
        let background_cg = background.CGColor();
        let _: () = unsafe { msg_send![layer, setBackgroundColor: &*background_cg] };
    });
}

fn place_icon_badge(mtm: MainThreadMarker, parent: &NSView, symbol: &str, x: f64, y: f64) {
    let Some(image) = system_symbol(symbol) else {
        return;
    };
    let badge = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(
            NSPoint::new(x, y),
            NSSize::new(ICON_BADGE_SIZE, ICON_BADGE_SIZE),
        ),
    );
    apply_layer_style(&badge, &badge_background_color(), ICON_BADGE_SIZE / 2.0);

    let icon_view = NSImageView::initWithFrame(
        NSImageView::alloc(mtm),
        NSRect::new(
            NSPoint::new(ICON_BADGE_INSET, ICON_BADGE_INSET),
            NSSize::new(
                ICON_BADGE_SIZE - ICON_BADGE_INSET * 2.0,
                ICON_BADGE_SIZE - ICON_BADGE_INSET * 2.0,
            ),
        ),
    );
    icon_view.setImage(Some(&image));
    icon_view.setImageScaling(NSImageScaling::ScaleProportionallyDown);
    icon_view.setSymbolConfiguration(Some(&symbol_configuration()));
    icon_view.setContentTintColor(Some(&secondary_text_color()));
    badge.addSubview(&icon_view);
    parent.addSubview(&badge);
}

fn menu_font() -> Retained<NSFont> {
    NSFont::systemFontOfSize(MENU_FONT_SIZE)
}

fn section_font() -> Retained<NSFont> {
    NSFont::boldSystemFontOfSize(SECTION_FONT_SIZE)
}

fn build_panel_content(
    mtm: MainThreadMarker,
    action_target: &PanelActionTarget,
    state: &TrayVoiceState,
    locale: &str,
    appearance: &NSAppearance,
) -> Retained<NSView> {
    let height = panel_height(state);
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PANEL_WIDTH, height));
    let container = NSView::initWithFrame(NSView::alloc(mtm), frame);
    let theme = VoiceTheme::from_appearance(appearance);
    configure_rounded_container(&container, &theme);

    let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
    configure_effect_view(&effect);
    container.addSubview(&effect);

    let content_width = PANEL_WIDTH - PANEL_PADDING * 2.0;
    let mut y = height - PANEL_PADDING;

    y = place_voice_card(
        mtm,
        &effect,
        action_target,
        state,
        y,
        content_width,
        &theme,
        locale,
    );

    y = place_menu_row(
        mtm,
        &effect,
        action_target,
        mezon_i18n::t(locale, "tray.showMezon"),
        "macwindow",
        action_tag::SHOW,
        false,
        y,
        content_width,
        &theme,
    );
    y = place_menu_row(
        mtm,
        &effect,
        action_target,
        mezon_i18n::t(locale, "tray.checkForUpdates"),
        "arrow.down.circle",
        action_tag::UPDATE,
        false,
        y,
        content_width,
        &theme,
    );
    let _ = place_menu_row(
        mtm,
        &effect,
        action_target,
        mezon_i18n::t(locale, "tray.quit"),
        "power",
        action_tag::QUIT,
        true,
        y,
        content_width,
        &theme,
    );

    container
}

fn place_voice_card(
    mtm: MainThreadMarker,
    parent: &NSVisualEffectView,
    target: &PanelActionTarget,
    state: &TrayVoiceState,
    mut y: f64,
    width: f64,
    theme: &VoiceTheme,
    locale: &str,
) -> f64 {
    let card_height = VOICE_CARD_HEIGHT;
    y -= card_height;
    let card = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(
            NSPoint::new(PANEL_PADDING, y),
            NSSize::new(width, card_height),
        ),
    );
    apply_layer_style(&card, &theme.card_background(), VOICE_CARD_CORNER_RADIUS);
    parent.addSubview(&card);

    let content_x = VOICE_CARD_PADDING;
    let content_width = width - VOICE_CARD_PADDING * 2.0;

    if state.in_call {
        let mut inner_y = card_height - VOICE_CARD_PADDING;

        let voice_section = mezon_i18n::t(locale, "tray.voiceSection").to_uppercase();
        inner_y = place_section_label(
            mtm,
            &card,
            &voice_section,
            inner_y,
            content_width,
            content_x,
        );
        let status = if state.camera_enabled {
            mezon_i18n::t(locale, "channelVoice.videoConnected")
        } else {
            mezon_i18n::t(locale, "channelVoice.voiceConnected")
        };
        inner_y = place_status_line(mtm, &card, status, inner_y, content_width, content_x, theme);
        if !state.channel_line.is_empty() {
            inner_y = place_subtitle_line(
                mtm,
                &card,
                &state.channel_line,
                inner_y,
                content_width,
                content_x,
            );
        }
        inner_y -= VOICE_CARD_SECTION_GAP;
        let _ = place_voice_controls(
            mtm,
            &card,
            target,
            state,
            inner_y,
            content_width,
            content_x,
            theme,
            locale,
        );
    } else {
        place_centered_idle_message(
            mtm,
            &card,
            card_height,
            content_width,
            content_x,
            theme,
            locale,
        );
    }

    y - VOICE_CARD_GAP
}

fn place_centered_idle_message(
    mtm: MainThreadMarker,
    parent: &NSView,
    card_height: f64,
    width: f64,
    x: f64,
    theme: &VoiceTheme,
    locale: &str,
) {
    let icon_color = theme.idle_status_color();
    let block_height = IDLE_ICON_SIZE + IDLE_ICON_MESSAGE_GAP + IDLE_MESSAGE_HEIGHT;
    let block_bottom = (card_height - block_height) / 2.0;
    let icon_x = x + (width - IDLE_ICON_SIZE) / 2.0;
    let icon_y = block_bottom + IDLE_MESSAGE_HEIGHT + IDLE_ICON_MESSAGE_GAP;

    if let Some(image) = system_symbol("speaker.wave.2.fill") {
        let icon_view = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(
                NSPoint::new(icon_x, icon_y),
                NSSize::new(IDLE_ICON_SIZE, IDLE_ICON_SIZE),
            ),
        );
        icon_view.setImage(Some(&image));
        icon_view.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        icon_view.setSymbolConfiguration(Some(&idle_voice_symbol_configuration(&icon_color)));
        icon_view.setContentTintColor(Some(&icon_color));
        parent.addSubview(&icon_view);
    }

    let label = NSTextField::labelWithString(
        &NSString::from_str(mezon_i18n::t(locale, "tray.noVoiceConnected")),
        mtm,
    );
    label.setEditable(false);
    label.setSelectable(false);
    label.setBezeled(false);
    label.setDrawsBackground(false);
    label.setAlignment(NSTextAlignment::Center);
    label.setTextColor(Some(&icon_color));
    label.setFont(Some(&NSFont::systemFontOfSize(IDLE_MESSAGE_FONT_SIZE)));
    label.setFrame(NSRect::new(
        NSPoint::new(x, block_bottom),
        NSSize::new(width, IDLE_MESSAGE_HEIGHT),
    ));
    parent.addSubview(&label);
}

fn place_section_label(
    mtm: MainThreadMarker,
    parent: &NSView,
    text: &str,
    mut y: f64,
    width: f64,
    x: f64,
) -> f64 {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setEditable(false);
    label.setSelectable(false);
    label.setBezeled(false);
    label.setDrawsBackground(false);
    label.setTextColor(Some(&section_text_color()));
    label.setFont(Some(&section_font()));
    y -= SECTION_LABEL_HEIGHT;
    label.setFrame(NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(width, SECTION_LABEL_HEIGHT),
    ));
    parent.addSubview(&label);
    y - SECTION_LABEL_GAP
}

fn place_status_line(
    mtm: MainThreadMarker,
    parent: &NSView,
    text: &str,
    mut y: f64,
    width: f64,
    x: f64,
    theme: &VoiceTheme,
) -> f64 {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setEditable(false);
    label.setSelectable(false);
    label.setBezeled(false);
    label.setDrawsBackground(false);
    label.setTextColor(Some(&theme.status_color()));
    label.setFont(Some(&NSFont::boldSystemFontOfSize(STATUS_FONT_SIZE)));
    y -= STATUS_LINE_HEIGHT;
    label.setFrame(NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(width, STATUS_LINE_HEIGHT),
    ));
    parent.addSubview(&label);
    y - STATUS_LINE_GAP
}

fn place_subtitle_line(
    mtm: MainThreadMarker,
    parent: &NSView,
    text: &str,
    mut y: f64,
    width: f64,
    x: f64,
) -> f64 {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setEditable(false);
    label.setSelectable(false);
    label.setBezeled(false);
    label.setDrawsBackground(false);
    label.setTextColor(Some(&secondary_text_color()));
    label.setFont(Some(&NSFont::systemFontOfSize(SUBTITLE_FONT_SIZE)));
    y -= SUBTITLE_LINE_HEIGHT;
    label.setFrame(NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(width, SUBTITLE_LINE_HEIGHT),
    ));
    parent.addSubview(&label);
    y - SUBTITLE_LINE_GAP
}

fn place_menu_row(
    mtm: MainThreadMarker,
    parent: &NSVisualEffectView,
    target: &PanelActionTarget,
    title: &str,
    symbol: &str,
    tag: isize,
    destructive: bool,
    mut y: f64,
    width: f64,
    theme: &VoiceTheme,
) -> f64 {
    y -= ROW_HEIGHT;
    let row = create_menu_row_view(
        mtm,
        NSRect::new(
            NSPoint::new(PANEL_PADDING, y),
            NSSize::new(width, ROW_HEIGHT),
        ),
        theme.is_dark,
    );
    parent.addSubview(&row);

    let badge_x = ROW_CONTENT_INSET;
    let badge_y = (ROW_HEIGHT - ICON_BADGE_SIZE) / 2.0;
    place_icon_badge(mtm, &row, symbol, badge_x, badge_y);

    let text_x = badge_x + ICON_BADGE_SIZE + ICON_TEXT_GAP;
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(panel_action_sel()),
            mtm,
        )
    };
    style_menu_button(&button, tag, destructive);
    button.setFrame(NSRect::new(
        NSPoint::new(text_x, 0.0),
        NSSize::new(width - text_x, ROW_HEIGHT),
    ));
    row.addSubview(&button);
    y - ROW_GAP
}

fn style_menu_button(button: &NSButton, tag: isize, destructive: bool) {
    button.setTag(tag);
    button.setBezelStyle(NSBezelStyle::Badge);
    button.setBordered(false);
    button.setTransparent(false);
    button.setFont(Some(&menu_font()));
    button.setAlignment(NSTextAlignment::Left);
    let tint = if destructive {
        NSColor::systemRedColor()
    } else {
        NSColor::labelColor()
    };
    button.setContentTintColor(Some(&tint));
}

fn place_voice_controls(
    mtm: MainThreadMarker,
    parent: &NSView,
    target: &PanelActionTarget,
    state: &TrayVoiceState,
    mut y: f64,
    width: f64,
    x: f64,
    theme: &VoiceTheme,
    locale: &str,
) -> f64 {
    y -= VOICE_CONTROL_SIZE;
    let controls = [
        (
            if state.mic_enabled {
                "mic.fill"
            } else {
                "mic.slash.fill"
            },
            mezon_i18n::t(locale, "tray.tooltip.microphone"),
            action_tag::MIC,
            state.mic_enabled,
            false,
        ),
        (
            if state.camera_enabled {
                "video.fill"
            } else {
                "video.slash.fill"
            },
            mezon_i18n::t(locale, "tray.tooltip.camera"),
            action_tag::CAMERA,
            state.camera_enabled,
            false,
        ),
        (
            if state.screen_enabled {
                "rectangle.inset.filled.and.person.filled"
            } else {
                "display"
            },
            mezon_i18n::t(locale, "tray.tooltip.screenShare"),
            action_tag::SCREEN,
            state.screen_enabled,
            false,
        ),
        (
            "phone.down.fill",
            mezon_i18n::t(locale, "tray.tooltip.leave"),
            action_tag::LEAVE,
            false,
            true,
        ),
    ];
    let button_width =
        (width - VOICE_CONTROL_GAP * (VOICE_CONTROL_COUNT - 1.0)) / VOICE_CONTROL_COUNT;
    for (index, (symbol, tooltip, tag, enabled, disconnect)) in controls.into_iter().enumerate() {
        let button_x = x + (button_width + VOICE_CONTROL_GAP) * index as f64;
        let Some(image) = system_symbol(symbol) else {
            continue;
        };
        let button = unsafe {
            NSButton::buttonWithImage_target_action(
                &image,
                Some(target),
                Some(panel_action_sel()),
                mtm,
            )
        };
        button.setTag(tag);
        button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        button.setBordered(false);
        button.setImageScaling(NSImageScaling::ScaleNone);
        button.setToolTip(Some(&NSString::from_str(tooltip)));
        let (background, icon_color) = voice_control_style(theme, enabled, disconnect);
        apply_layer_style(&button, &background, VOICE_CONTROL_SIZE / 2.0);
        button.setSymbolConfiguration(Some(&voice_control_symbol_configuration(&icon_color)));
        button.setContentTintColor(Some(&icon_color));
        button.setFrame(NSRect::new(
            NSPoint::new(button_x, y),
            NSSize::new(button_width, VOICE_CONTROL_SIZE),
        ));
        parent.addSubview(&button);
    }
    y - ROW_GAP
}

fn panel_height(_state: &TrayVoiceState) -> f64 {
    let menu_block = PANEL_PADDING
        + ROW_HEIGHT * MENU_ROW_COUNT
        + ROW_GAP * (MENU_ROW_COUNT - 1.0)
        + PANEL_PADDING;
    VOICE_CARD_HEIGHT + VOICE_CARD_GAP + menu_block
}

fn main_screen_scale() -> f64 {
    MainThreadMarker::new()
        .and_then(NSScreen::mainScreen)
        .map(|screen| screen.backingScaleFactor())
        .unwrap_or(DEFAULT_SCREEN_SCALE)
}

fn screen_height_points() -> f64 {
    MainThreadMarker::new()
        .and_then(NSScreen::mainScreen)
        .map(|screen| screen.frame().size.height)
        .unwrap_or_else(|| CGDisplayPixelsHigh(CGMainDisplayID()) as f64 / main_screen_scale())
}

fn build_tray_icon(base: &RgbaImage, in_call: bool) -> Result<Icon> {
    let mut rgba = base.clone();
    if in_call {
        draw_call_indicator(&mut rgba);
    }
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).map_err(Into::into)
}
