use crate::placement::PickerPlacement;
use crate::protocol::{
    AttributionQuality, Candidate, ClipdFrame, IpcPeer, MAX_OPEN_REQUEST_BYTES, OpenRequest,
    PickerTx, RealmKind, display_content_kind, read_bounded_line, sanitize_preview,
};
use base64::Engine;
use gtk4::gdk::prelude::MonitorExt;
use gtk4::prelude::*;
use gtk4::{Align, Orientation, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use log::{info, warn};
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemePalette {
    pub background: String,
    pub foreground: String,
    pub border: String,
    pub accent: String,
    pub selected_background: String,
    pub realm_background: String,
    pub search_background: String,
    pub warning_background: String,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self {
            background: "#1e1e2e".to_owned(),
            foreground: "#f8f8f2".to_owned(),
            border: "#89b4fa".to_owned(),
            accent: "#3584e4".to_owned(),
            selected_background: "alpha(#3584e4, 0.14)".to_owned(),
            realm_background: "alpha(#3584e4, 0.14)".to_owned(),
            search_background: "alpha(currentColor, 0.07)".to_owned(),
            warning_background: "alpha(#f5c211, 0.22)".to_owned(),
        }
    }
}

impl ThemePalette {
    pub fn from_json_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        let palette: Self = serde_json::from_str(&text)?;
        palette.validate()?;
        Ok(palette)
    }

    fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        for (name, value) in [
            ("background", &self.background),
            ("foreground", &self.foreground),
            ("border", &self.border),
            ("accent", &self.accent),
            ("selected_background", &self.selected_background),
            ("realm_background", &self.realm_background),
            ("search_background", &self.search_background),
            ("warning_background", &self.warning_background),
        ] {
            if !is_safe_css_color(value) {
                return Err(format!(
                    "theme field {name} must be a #rrggbb color or alpha(#rrggbb|currentColor, opacity)"
                )
                .into());
            }
        }
        Ok(())
    }

    fn css(&self) -> String {
        format!(
            "
        window.d2b-clip-picker {{
            background-color: {background};
            color: {foreground};
            border: 2px solid {border};
            border-radius: 12px;
        }}
        .d2b-clip-picker-root {{
            background-color: {background};
            color: {foreground};
            border: 2px solid {border};
            border-radius: 12px;
        }}
        headerbar {{ background: transparent; box-shadow: none; }}
        .clipboard-list {{ background: transparent; }}
        .clipboard-item {{
            border: 2px solid transparent;
            border-radius: 10px;
            padding: 4px;
            margin: 6px 12px;
            transition: border-color 150ms ease, background 150ms ease;
        }}
        .clipboard-item:hover {{ border-color: {accent}; }}
        .clipboard-item:selected {{ border-color: {accent}; background: {selected_background}; }}
        .clipboard-preview {{ opacity: 0.94; }}
        .realm-pill, .search-pill, .warning-pill {{
            border-radius: 999px;
            padding: 4px 8px;
        }}
        .realm-pill {{ background: {realm_background}; }}
        .search-pill {{ background: {search_background}; }}
        .warning-pill {{ background: {warning_background}; }}
        ",
            background = self.background,
            foreground = self.foreground,
            border = self.border,
            accent = self.accent,
            selected_background = self.selected_background,
            realm_background = self.realm_background,
            search_background = self.search_background,
            warning_background = self.warning_background,
        )
    }
}

pub fn run_picker(
    request: OpenRequest,
    peer: IpcPeer,
    placement: PickerPlacement,
    test_select_first: bool,
    theme: ThemePalette,
) -> Result<(), Box<dyn std::error::Error>> {
    adw::init()?;
    let tx = peer.tx_for_request(&request);
    let (socket_closed_tx, socket_closed_rx) = mpsc::channel();
    let mut reader = peer.into_reader();
    std::thread::spawn(
        move || match read_bounded_line(&mut reader, MAX_OPEN_REQUEST_BYTES) {
            Err(_) => {
                let _ = socket_closed_tx.send(());
            }
            Ok(line) => {
                let _ = serde_json::from_str::<ClipdFrame>(line.trim_end());
                let _ = socket_closed_tx.send(());
            }
        },
    );

    let app: gtk4::Application = adw::Application::builder()
        .application_id("io.github.vicondoa.d2b_clip_picker")
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build()
        .upcast();

    let request_for_activate = request.clone();
    let placement_for_activate = placement.clone();
    let theme_for_activate = theme.clone();
    app.connect_activate(move |app| {
        let window = create_window(
            app,
            request_for_activate.clone(),
            tx.clone(),
            placement_for_activate.clone(),
            test_select_first,
            theme_for_activate.clone(),
        );
        window.present();
    });

    let app_for_socket = app.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if socket_closed_rx.try_recv().is_ok() {
            app_for_socket.quit();
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

    app.run_with_args::<String>(&[]);
    Ok(())
}

fn create_window(
    app: &gtk4::Application,
    request: OpenRequest,
    tx: PickerTx,
    mut placement: PickerPlacement,
    test_select_first: bool,
    theme: ThemePalette,
) -> adw::ApplicationWindow {
    configure_color_scheme();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("d2b clipboard picker")
        .decorated(false)
        .default_width(420)
        .default_height(520)
        .build();
    window.add_css_class("d2b-clip-picker");
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_namespace(Some("d2b-clip-picker"));
    let monitor = placement
        .output
        .as_deref()
        .and_then(find_monitor)
        .or_else(default_monitor);
    if let Some(monitor) = monitor.as_ref() {
        if placement.geometry.output_width <= 0 || placement.geometry.output_height <= 0 {
            let geometry = monitor.geometry();
            placement.geometry.output_width = geometry.width();
            placement.geometry.output_height = geometry.height();
            placement.geometry.x =
                ((geometry.width() - placement.geometry.overlay_width).max(16) / 2) as f64;
            placement.geometry.y =
                ((geometry.height() - placement.geometry.overlay_height).max(16) / 2) as f64;
        }
        window.set_monitor(Some(monitor));
    }
    info!(
        "picker window placement x={} y={} overlay_width={} overlay_height={} output={:?}",
        placement.geometry.x,
        placement.geometry.y,
        placement.geometry.overlay_width,
        placement.geometry.overlay_height,
        placement.output
    );
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    let placement = placement.geometry;
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_margin(Edge::Top, placement.y as i32);
    window.set_margin(Edge::Left, placement.x as i32);

    if placement.output_width > 0 && placement.output_height > 0 {
        window.connect_map(move |mapped| {
            let mapped = mapped.clone();
            glib::idle_add_local_once(move || {
                let margin = 8.0;
                let width = mapped.allocated_width().max(placement.overlay_width) as f64;
                let height = mapped.allocated_height().max(placement.overlay_height) as f64;
                let max_x = (placement.output_width as f64 - width - margin).max(margin);
                let max_y = (placement.output_height as f64 - height - margin).max(margin);
                mapped.set_margin(Edge::Top, placement.y.clamp(margin, max_y) as i32);
                mapped.set_margin(Edge::Left, placement.x.clamp(margin, max_x) as i32);
            });
        });
    }

    apply_css(&window, &theme);
    let sent_terminal = Rc::new(Cell::new(false));
    let confirm_entry = Rc::new(RefCell::new(None::<String>));
    let search = Rc::new(RefCell::new(String::new()));
    let displayed = Rc::new(RefCell::new(Vec::<Candidate>::new()));

    let main_box = gtk4::Box::new(Orientation::Vertical, 0);
    main_box.add_css_class("d2b-clip-picker-root");
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.set_show_start_title_buttons(false);
    header.set_title_widget(Some(&gtk4::Label::new(Some("d2b clipboard picker"))));

    let close_button = gtk4::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Cancel")
        .build();
    close_button.add_css_class("flat");
    header.pack_end(&close_button);
    main_box.append(&header);

    let destination = gtk4::Label::new(Some(&destination_label(&request)));
    destination.add_css_class("title-4");
    destination.set_halign(Align::Start);
    destination.set_margin_start(16);
    destination.set_margin_end(16);
    destination.set_margin_top(8);
    destination.set_wrap(true);
    main_box.append(&destination);

    let requested = gtk4::Label::new(Some(&format!(
        "Requested {} · type to filter",
        request.requested_mime_type
    )));
    requested.add_css_class("caption");
    requested.add_css_class("dim-label");
    requested.set_halign(Align::Start);
    requested.set_margin_start(16);
    requested.set_margin_end(16);
    main_box.append(&requested);

    let search_label = gtk4::Label::new(Some("Search…"));
    search_label.add_css_class("search-pill");
    search_label.set_halign(Align::Fill);
    search_label.set_margin_top(10);
    search_label.set_margin_bottom(6);
    search_label.set_margin_start(16);
    search_label.set_margin_end(16);
    main_box.append(&search_label);

    let banner = gtk4::Label::new(None);
    banner.add_css_class("warning-pill");
    banner.set_visible(false);
    banner.set_margin_start(16);
    banner.set_margin_end(16);
    main_box.append(&banner);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_min_content_width(placement.overlay_width);
    scrolled.set_min_content_height(placement.overlay_height);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("clipboard-list");
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    scrolled.set_child(Some(&list_box));
    main_box.append(&scrolled);
    window.set_content(Some(&main_box));

    rebuild_rows(&list_box, &request.candidates, &search.borrow(), &displayed);
    if let Some(first) = list_box.row_at_index(0) {
        list_box.select_row(Some(&first));
    }

    let app_for_close = app.clone();
    let tx_for_close = tx.clone();
    let sent_for_close = sent_terminal.clone();
    close_button.connect_clicked(move |_| {
        send_cancel_once(&tx_for_close, &sent_for_close);
        app_for_close.quit();
    });

    let app_for_close_request = app.clone();
    let tx_for_close_request = tx.clone();
    let sent_for_close_request = sent_terminal.clone();
    window.connect_close_request(move |_| {
        send_cancel_once(&tx_for_close_request, &sent_for_close_request);
        app_for_close_request.quit();
        glib::Propagation::Stop
    });

    let activation = ActivationContext {
        tx: tx.clone(),
        app: app.clone(),
        sent_terminal: sent_terminal.clone(),
        confirm_entry: confirm_entry.clone(),
        banner: banner.clone(),
    };
    if test_select_first {
        let displayed_for_test = displayed.clone();
        let activation_for_test = activation.clone();
        window.connect_map(move |_| {
            let displayed_for_test = displayed_for_test.clone();
            let activation_for_test = activation_for_test.clone();
            glib::idle_add_local_once(move || {
                if let Some(candidate) = displayed_for_test.borrow().first() {
                    info!(
                        "test-select-first mapped; selecting entry {}",
                        candidate.entry_id
                    );
                    activate_candidate(candidate, &activation_for_test);
                }
            });
        });
    }
    let displayed_for_activation = displayed.clone();
    let activation_for_click = activation.clone();
    list_box.connect_row_activated(move |_, row| {
        if let Some(candidate) = displayed_for_activation.borrow().get(row.index() as usize) {
            activate_candidate(candidate, &activation_for_click);
        }
    });
    let click_controller = gtk4::GestureClick::new();
    let list_for_click = list_box.clone();
    let displayed_for_click_select = displayed.clone();
    let activation_for_single_click = activation.clone();
    click_controller.connect_released(move |_, _, _, _| {
        if let Some(row) = list_for_click.selected_row()
            && let Some(candidate) = displayed_for_click_select
                .borrow()
                .get(row.index() as usize)
        {
            activate_candidate(candidate, &activation_for_single_click);
        }
    });
    list_box.add_controller(click_controller);

    let key_controller = gtk4::EventControllerKey::new();
    let list_for_keys = list_box.clone();
    let request_for_keys = request.clone();
    let displayed_for_keys = displayed.clone();
    let search_for_keys = search.clone();
    let search_label_for_keys = search_label.clone();
    let activation_for_keys = activation.clone();
    key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
        use gdk::Key;
        if modifiers.contains(gdk::ModifierType::CONTROL_MASK) && key == Key::v {
            return glib::Propagation::Stop;
        }
        match key {
            Key::Escape => {
                send_cancel_once(&activation_for_keys.tx, &activation_for_keys.sent_terminal);
                activation_for_keys.app.quit();
                glib::Propagation::Stop
            }
            Key::Down | Key::j | Key::J => {
                select_relative(&list_for_keys, 1);
                glib::Propagation::Stop
            }
            Key::Up | Key::k | Key::K => {
                select_relative(&list_for_keys, -1);
                glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter => {
                if let Some(row) = list_for_keys.selected_row()
                    && let Some(candidate) = displayed_for_keys.borrow().get(row.index() as usize)
                {
                    activate_candidate(candidate, &activation_for_keys);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            Key::BackSpace => {
                search_for_keys.borrow_mut().pop();
                update_search(
                    &search_for_keys,
                    &search_label_for_keys,
                    &list_for_keys,
                    &request_for_keys.candidates,
                    &displayed_for_keys,
                );
                glib::Propagation::Stop
            }
            Key::Delete => glib::Propagation::Stop,
            _ => {
                if let Some(ch) = key.to_unicode()
                    && !ch.is_control()
                {
                    search_for_keys.borrow_mut().push(ch);
                    update_search(
                        &search_for_keys,
                        &search_label_for_keys,
                        &list_for_keys,
                        &request_for_keys.candidates,
                        &displayed_for_keys,
                    );
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        }
    });
    window.add_controller(key_controller);

    window
}

#[derive(Clone)]
struct ActivationContext {
    tx: PickerTx,
    app: gtk4::Application,
    sent_terminal: Rc<Cell<bool>>,
    confirm_entry: Rc<RefCell<Option<String>>>,
    banner: gtk4::Label,
}

fn activate_candidate(candidate: &Candidate, ctx: &ActivationContext) {
    if ctx.sent_terminal.get() {
        return;
    }
    if candidate.confirmation_required {
        let mut pending = ctx.confirm_entry.borrow_mut();
        if pending.as_deref() != Some(candidate.entry_id.as_str()) {
            *pending = Some(candidate.entry_id.clone());
            ctx.banner.set_text("Select this item again to confirm.");
            ctx.banner.set_visible(true);
            return;
        }
    }
    ctx.sent_terminal.set(true);
    if let Err(_err) = ctx.tx.select(&candidate.entry_id) {
        warn!("failed to send selection to d2b-clipd");
    }
    ctx.app.quit();
}

fn send_cancel_once(tx: &PickerTx, sent_terminal: &Rc<Cell<bool>>) {
    if sent_terminal.replace(true) {
        return;
    }
    if let Err(_err) = tx.cancel() {
        warn!("failed to send cancellation to d2b-clipd");
    }
}

fn update_search(
    search: &Rc<RefCell<String>>,
    label: &gtk4::Label,
    list_box: &gtk4::ListBox,
    candidates: &[Candidate],
    displayed: &Rc<RefCell<Vec<Candidate>>>,
) {
    let query = search.borrow();
    if query.is_empty() {
        label.set_text("Search…");
    } else {
        label.set_text(&format!("Search: {query}"));
    }
    rebuild_rows(list_box, candidates, &query, displayed);
    if let Some(first) = list_box.row_at_index(0) {
        list_box.select_row(Some(&first));
    }
}

fn find_monitor(output: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    for index in 0..monitors.n_items() {
        let Some(item) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = item.downcast::<gdk::Monitor>() else {
            continue;
        };
        let connector = monitor.connector();
        let model = monitor.model();
        let matches = connector
            .as_ref()
            .is_some_and(|connector| connector == output)
            || model.as_ref().is_some_and(|model| model.contains(output));
        if matches {
            return Some(monitor);
        }
    }
    None
}

fn default_monitor() -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    monitors.item(0)?.downcast::<gdk::Monitor>().ok()
}

fn rebuild_rows(
    list_box: &gtk4::ListBox,
    candidates: &[Candidate],
    query: &str,
    displayed: &Rc<RefCell<Vec<Candidate>>>,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    let query = query.to_lowercase();
    let mut visible = Vec::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate_matches(candidate, &query))
    {
        visible.push(candidate.clone());
        list_box.append(&candidate_row(candidate));
    }
    if visible.is_empty() {
        let row = gtk4::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let label = gtk4::Label::new(Some("No matching clipboard entries"));
        label.add_css_class("dim-label");
        label.set_margin_top(24);
        label.set_margin_bottom(24);
        row.set_child(Some(&label));
        list_box.append(&row);
    }
    *displayed.borrow_mut() = visible;
}

fn candidate_matches(candidate: &Candidate, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {} {} {}",
        candidate.source_realm,
        candidate.source_app.as_deref().unwrap_or_default(),
        candidate.source_app_id.as_deref().unwrap_or_default(),
        candidate.content_type,
        candidate.preview_text.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    haystack.contains(query)
}

fn candidate_row(candidate: &Candidate) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.add_css_class("clipboard-item");
    let main = gtk4::Box::new(Orientation::Vertical, 6);
    main.set_margin_top(10);
    main.set_margin_bottom(10);
    main.set_margin_start(12);
    main.set_margin_end(12);

    let header = gtk4::Box::new(Orientation::Horizontal, 8);
    let (kind, icon) = display_content_kind(&candidate.content_type);
    let icon_label = gtk4::Label::new(Some(icon));
    let realm = gtk4::Label::new(Some(&source_label(candidate)));
    realm.add_css_class("realm-pill");
    let kind_label = gtk4::Label::new(Some(kind));
    kind_label.add_css_class("caption");
    kind_label.set_halign(Align::Start);
    kind_label.set_hexpand(true);
    let time = gtk4::Label::new(Some(&format_timestamp(candidate.timestamp_unix_ms)));
    time.add_css_class("caption");
    time.add_css_class("dim-label");
    header.append(&icon_label);
    header.append(&realm);
    header.append(&kind_label);
    header.append(&time);
    main.append(&header);

    let app_label = gtk4::Label::new(Some(&app_source_label(candidate)));
    app_label.add_css_class("caption");
    app_label.add_css_class("dim-label");
    app_label.set_halign(Align::Start);
    app_label.set_wrap(true);
    main.append(&app_label);

    if let Some(texture) = safe_host_thumbnail(candidate) {
        let picture = gtk4::Picture::for_paintable(&texture);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_height_request(160);
        picture.set_halign(Align::Center);
        picture.add_css_class("clipboard-preview");
        main.append(&picture);
    } else {
        let preview = preview_text(candidate);
        let preview_label = gtk4::Label::new(Some(&preview));
        preview_label.add_css_class("clipboard-preview");
        preview_label.set_halign(Align::Start);
        preview_label.set_wrap(true);
        preview_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        preview_label.set_max_width_chars(58);
        preview_label.set_lines(4);
        preview_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        main.append(&preview_label);
    }

    if candidate.confirmation_required {
        let confirm = gtk4::Label::new(Some("Confirmation required"));
        confirm.add_css_class("warning-pill");
        confirm.set_halign(Align::Start);
        main.append(&confirm);
    }

    row.set_child(Some(&main));
    row
}

fn destination_label(request: &OpenRequest) -> String {
    let realm = sanitize_preview(&request.destination.realm, 80);
    let app = request
        .destination
        .application
        .as_deref()
        .or(request.destination.app_id.as_deref())
        .map(|value| sanitize_preview(value, 80));
    match app {
        Some(app) if !app.is_empty() => format!("Paste into {realm} · {app}"),
        _ => format!("Paste into {realm}"),
    }
}

fn source_label(candidate: &Candidate) -> String {
    let realm = sanitize_preview(&candidate.source_realm, 48);
    match candidate.source_realm_kind {
        RealmKind::Host => realm,
        RealmKind::Vm => format!("{realm} VM"),
        RealmKind::Unknown => realm,
    }
}

fn app_source_label(candidate: &Candidate) -> String {
    let app = candidate
        .source_app
        .as_deref()
        .or(candidate.source_app_id.as_deref())
        .unwrap_or("Unknown app");
    let app = sanitize_preview(app, 80);
    match candidate.source_attribution {
        AttributionQuality::ExactClient => format!("Exact source · {app}"),
        AttributionQuality::FocusedWindowGuess => format!("Focused-window guess · {app}"),
        AttributionQuality::CacheStaleFocusedWindowGuess => {
            format!("Focused-window guess (stale) · {app}")
        }
        AttributionQuality::BrokerInjectedDebug => "Debug-injected source".to_owned(),
    }
}

fn preview_text(candidate: &Candidate) -> String {
    if candidate.content_type.split(';').next() == Some("image/png") {
        let bytes = candidate
            .byte_count
            .map(|count| format!(" · {count} bytes"))
            .unwrap_or_default();
        return format!("Image entry{bytes}");
    }
    sanitize_preview(
        candidate.preview_text.as_deref().unwrap_or("No preview"),
        300,
    )
}

fn safe_host_thumbnail(candidate: &Candidate) -> Option<gdk::Texture> {
    if candidate.source_realm_kind != RealmKind::Host {
        return None;
    }
    if candidate.content_type.split(';').next()? != "image/png" {
        return None;
    }
    let encoded = candidate.thumbnail_png_base64.as_ref()?;
    if encoded.len() > 256 * 1024 {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if decoded.len() > 192 * 1024 {
        return None;
    }
    let (width, height) = png_dimensions(&decoded)?;
    if width > 4096 || height > 4096 || width.saturating_mul(height) > 8_000_000 {
        return None;
    }
    let bytes = glib::Bytes::from_owned(decoded);
    gdk::Texture::from_bytes(&bytes).ok()
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIG {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn select_relative(list_box: &gtk4::ListBox, delta: i32) {
    let current = list_box.selected_row().map(|row| row.index()).unwrap_or(0);
    let next = (current + delta).max(0);
    if let Some(row) = list_box.row_at_index(next) {
        list_box.select_row(Some(&row));
        row.grab_focus();
    }
}

fn format_timestamp(timestamp_ms: Option<u64>) -> String {
    let Some(timestamp_ms) = timestamp_ms else {
        return "Unknown time".to_owned();
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(timestamp_ms);
    let diff = now_ms.saturating_sub(timestamp_ms) / 1000;
    if diff < 30 {
        "Just now".to_owned()
    } else if diff < 3600 {
        let minutes = diff / 60;
        format!("{minutes}m ago")
    } else if diff < 86_400 {
        let hours = diff / 3600;
        format!("{hours}h ago")
    } else {
        let days = diff / 86_400;
        format!("{days}d ago")
    }
}

fn configure_color_scheme() {
    let style_manager = adw::StyleManager::default();
    if let Some(settings) = gtk4::Settings::default() {
        if settings.is_gtk_application_prefer_dark_theme() {
            style_manager.set_color_scheme(adw::ColorScheme::PreferDark);
        } else {
            style_manager.set_color_scheme(adw::ColorScheme::Default);
        }
    }
}

fn apply_css(window: &adw::ApplicationWindow, theme: &ThemePalette) {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(&theme.css());
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn is_safe_css_color(value: &str) -> bool {
    is_hex_color(value) || is_alpha_color(value)
}

fn is_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7
        && bytes[0] == b'#'
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn is_alpha_color(value: &str) -> bool {
    let Some(inner) = value
        .strip_prefix("alpha(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let Some((color, opacity)) = inner.split_once(',') else {
        return false;
    };
    let color = color.trim();
    let opacity = opacity.trim();
    (color == "currentColor" || is_hex_color(color))
        && opacity
            .parse::<f32>()
            .is_ok_and(|parsed| (0.0..=1.0).contains(&parsed))
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn default_palette_matches_existing_visual_contract() {
        let css = ThemePalette::default().css();
        assert!(css.contains("background-color: #1e1e2e;"));
        assert!(css.contains("border: 2px solid #89b4fa;"));
        assert!(css.contains("background: alpha(#3584e4, 0.14);"));
    }

    #[test]
    fn theme_palette_accepts_safe_colors() {
        let palette = ThemePalette {
            background: "#010203".to_owned(),
            foreground: "#111213".to_owned(),
            border: "#212223".to_owned(),
            accent: "#313233".to_owned(),
            selected_background: "alpha(#313233, 0.14)".to_owned(),
            realm_background: "alpha(#313233, 0.14)".to_owned(),
            search_background: "alpha(currentColor, 0.07)".to_owned(),
            warning_background: "alpha(#414243, 1)".to_owned(),
        };
        palette.validate().expect("palette should validate");
    }

    #[test]
    fn theme_palette_rejects_uppercase_hex_and_arbitrary_css() {
        let mut palette = ThemePalette {
            background: "#ABCDEF".to_owned(),
            ..ThemePalette::default()
        };
        assert!(palette.validate().is_err());

        palette.background = "url(file:///tmp/x)".to_owned();
        assert!(palette.validate().is_err());
    }
}
