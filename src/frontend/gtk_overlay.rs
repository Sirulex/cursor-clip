use crate::shared::config::{PopupPosition, UserConfig, config_path};
use crate::shared::{ClipboardContentType, ClipboardItemPreview};
use cursor_clip::ipc_client::{FrontendClient, PersistenceUpdateError};
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box, Button, CheckButton, Label, Orientation, Overlay, Revealer,
    SearchEntry,
};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use log::{debug, error, info, warn};
use std::cell::{Cell, RefCell};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::rc::Rc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::frontend::hyprland;
use crate::frontend::toggle::ToggleServer;
use std::os::fd::AsRawFd;

use super::OverlayLocation;

static INIT: Once = Once::new();
pub static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

const OVERLAY_MIN_WIDTH: i32 = 200;

// Thread-local storage for the overlay state since GTK objects aren't Send/Sync
thread_local! {
    static OVERLAY_WINDOW: RefCell<Option<adw::ApplicationWindow>> = const { RefCell::new(None) };
    static OVERLAY_APP: RefCell<Option<Application>> = const { RefCell::new(None) };
    // Set when the overlay stays alive between uses (hidden instead of quit).
    static RESIDENT: RefCell<Option<ResidentOverlay>> = const { RefCell::new(None) };
    static PERSISTENCE_PENDING: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone)]
struct HistoryListState {
    // Ordered like the list rows; search hides rows through the filter instead
    // of rebuilding them, so built rows and their layout stay alive.
    all_items: Rc<RefCell<Vec<ClipboardItemPreview>>>,
    search_query: Rc<RefCell<String>>,
    show_trash: Rc<RefCell<bool>>,
    show_pin: Rc<RefCell<bool>>,
    // Bumped on every rebuild so an unfinished incremental fill stops.
    render_generation: Rc<Cell<u64>>,
    // A restarted daemon can reuse IDs while the resident window is hidden.
    current_snapshot: Rc<Cell<bool>>,
    placeholder: Label,
}

impl HistoryListState {
    fn can_act_on_rows(&self) -> bool {
        self.current_snapshot.get() && !persistence_pending()
    }
}

struct PersistenceControls {
    check: CheckButton,
    spinner: gtk4::Spinner,
    status: Label,
    list_box: gtk4::ListBox,
    clear_button: Button,
    history_state: HistoryListState,
    history_status: Label,
    refresh_generation: Cell<u64>,
    confirmed: Cell<Option<bool>>,
    checking: Cell<bool>,
}

struct OverlayContent {
    persistence: Rc<PersistenceControls>,
    overlay: Overlay,
    list_size: Rc<ListSize>,
    list_box: gtk4::ListBox,
    history_state: HistoryListState,
    search_entry: SearchEntry,
    menu_revealer: Revealer,
}

/// The built overlay: window plus the parts that are reset on every show.
struct OverlayUi {
    window: adw::ApplicationWindow,
    backdrop: Overlay,
    list_size: Rc<ListSize>,
    list_box: gtk4::ListBox,
    history_state: HistoryListState,
    search_entry: SearchEntry,
    menu_revealer: Revealer,
    placement: Rc<Cell<CardPlacement>>,
}

/// A hidden overlay kept in memory with its rows already laid out, so the
/// next open only has to apply history changes and map the window.
struct ResidentOverlay {
    ui: OverlayUi,
    _hold: gtk4::gio::ApplicationHoldGuard,
}

fn persistence_pending() -> bool {
    PERSISTENCE_PENDING.with(Cell::get)
}

fn open_history(controls: &Rc<PersistenceControls>) {
    controls.history_state.current_snapshot.set(false);
    let list = &controls.list_box;
    if !select_first_row_with_focus(list, true)
        && let Some(root) = list.root()
    {
        // An empty ListBox has no child for its grab_focus implementation.
        // Keep logical focus here until the first asynchronous snapshot arrives.
        root.set_focus(Some(list));
    }
    if !persistence_pending() {
        update_persistence(controls.clone(), None);
        refresh_history(controls.clone());
    }
}

fn refresh_history(controls: Rc<PersistenceControls>) {
    let generation = controls.refresh_generation.get().wrapping_add(1);
    controls.refresh_generation.set(generation);
    gtk4::glib::spawn_future_local(async move {
        loop {
            let rendered = controls.history_state.render_generation.get();
            let result = FrontendClient::history().await;
            if controls.refresh_generation.get() != generation || persistence_pending() {
                return;
            }
            match result {
                Ok(_) if rendered != controls.history_state.render_generation.get() => {
                    // A pin, delete or expiry changed the list after this request started.
                    continue;
                }
                Ok(items) => {
                    apply_history(&controls.list_box, &controls.history_state, items);
                    controls.history_state.current_snapshot.set(true);
                    controls.history_status.set_visible(false);
                }
                Err(error) => {
                    warn!("Failed to refresh clipboard history: {error}");
                    controls
                        .history_status
                        .set_label("Could not refresh clipboard history. Reopen to retry.");
                    controls.history_status.set_visible(true);
                }
            }
            break;
        }
    });
}

fn apply_history(list: &gtk4::ListBox, state: &HistoryListState, items: Vec<ClipboardItemPreview>) {
    let focused = list.root().and_then(|root| root.focus());
    let focus_row = focused.as_ref().is_some_and(|widget| {
        widget == list.upcast_ref::<gtk4::Widget>()
            || widget.is::<gtk4::ListBoxRow>() && widget.is_ancestor(list)
    });
    // On reopen the first row should be the newest item in the new snapshot.
    // During an in-place refresh, keep the user's existing selection.
    let selected = state
        .current_snapshot
        .get()
        .then(|| selected_item_id(list))
        .flatten();
    *state.all_items.borrow_mut() = items;
    rebuild_list(list, state);
    if let Some(row) = selected.and_then(|id| {
        list_rows(list)
            .into_iter()
            .find(|row| row_item_id(row) == Some(id) && is_row_shown(row))
    }) {
        list.select_row(Some(&row));
    }
    if focus_row && let Some(row) = list.selected_row() {
        row.grab_focus();
    }
}

// A read-only query leaves keyboard navigation and shutdown available.
// Only a user-requested transition gates actions that need the backend lock.
fn update_persistence(controls: Rc<PersistenceControls>, enabled: Option<bool>) {
    if controls.checking.replace(true) {
        return;
    }
    let changing = enabled.is_some();
    if changing {
        PERSISTENCE_PENDING.with(|pending| pending.set(true));
        controls.list_box.set_sensitive(false);
        controls.clear_button.set_sensitive(false);
        controls
            .refresh_generation
            .set(controls.refresh_generation.get().wrapping_add(1));
    }
    controls.check.set_sensitive(false);
    controls.status.remove_css_class("error");
    controls.status.set_label(if changing {
        "Updating history storage…"
    } else {
        "Checking history storage…"
    });
    controls.status.set_visible(true);
    controls.spinner.set_visible(true);
    controls.spinner.start();

    gtk4::glib::spawn_future_local(async move {
        match FrontendClient::persistence(enabled).await {
            Ok(enabled) => {
                controls.confirmed.set(Some(enabled));
                controls.check.set_inconsistent(false);
                controls.check.set_active(enabled);
                controls.status.set_visible(false);
            }
            Err(error) => {
                let message = match error {
                    PersistenceUpdateError::Rejected(error) if changing => {
                        warn!("Daemon rejected persistence change: {error}");
                        if let Some(previous) = controls.confirmed.get() {
                            controls.check.set_active(previous);
                        }
                        "Could not change history storage."
                    }
                    PersistenceUpdateError::Rejected(error)
                    | PersistenceUpdateError::Unconfirmed(error) => {
                        warn!("Could not confirm history storage: {error}");
                        controls.confirmed.set(None);
                        if changing {
                            controls.history_state.current_snapshot.set(false);
                        }
                        "Could not confirm history storage. Reopen to check again."
                    }
                };
                controls
                    .check
                    .set_inconsistent(controls.confirmed.get().is_none());
                controls.status.set_label(message);
                controls.status.add_css_class("error");
            }
        }
        controls.spinner.stop();
        controls.spinner.set_visible(false);
        controls.checking.set(false);
        controls
            .check
            .set_sensitive(controls.confirmed.get().is_some());
        if changing {
            PERSISTENCE_PENDING.with(|pending| pending.set(false));
            controls.list_box.set_sensitive(true);
            controls.clear_button.set_sensitive(true);
            if is_close_requested() {
                request_quit();
            } else {
                refresh_history(controls);
            }
        }
    });
}

pub(super) fn overlay_config(path: &Path) -> Option<UserConfig> {
    match UserConfig::load_or_create(path) {
        Ok(config) => Some(config),
        Err(e) => {
            eprintln!(
                "cursor-clip: cannot load {}. Using temporary display settings until the file is corrected.\n{e}",
                path.display()
            );
            None
        }
    }
}

pub fn is_close_requested() -> bool {
    CLOSE_REQUESTED.load(Ordering::Relaxed)
}

pub fn reset_close_flags() {
    CLOSE_REQUESTED.store(false, Ordering::Relaxed);
}

// Centralized quit path to avoid double-close reentrancy and ensure flags + app quit
fn request_quit() {
    // A resident overlay is only hidden; it stays ready for the next open.
    if RESIDENT.with(|resident| resident.borrow().is_some()) {
        hide_resident_overlay();
        return;
    }

    CLOSE_REQUESTED.store(true, Ordering::Relaxed);
    if persistence_pending() {
        OVERLAY_WINDOW.with(|window| {
            if let Some(window) = window.borrow().as_ref() {
                window.set_visible(false);
            }
        });
        return;
    }
    // Prefer quitting the application (cleaner teardown) over closing the window directly
    OVERLAY_APP.with(|a| {
        if let Some(ref app) = *a.borrow() {
            app.quit();
        }
    });

    // Fallback: close the window if app is unavailable
    OVERLAY_WINDOW.with(|w| {
        if let Some(ref win) = *w.borrow() {
            win.close();
        }
    });
}

/// Run the overlay. With `resident`, closing hides the window and later
/// toggle requests show it again instead of starting a new process.
pub(super) fn init_clipboard_overlay(
    location: OverlayLocation,
    toggle_listener: UnixListener,
    resident: bool,
    config: UserConfig,
) -> Result<(), std::boxed::Box<dyn std::error::Error + Send + Sync>> {
    INIT.call_once(|| {
        adw::init().expect("Failed to initialize libadwaita");
    });
    configure_color_scheme();

    // Create the application (was returned from init_application())
    let app: Application = adw::Application::builder()
        .application_id("io.github.sirulex.cursor-clip")
        .build()
        .upcast();

    let app_clone = app.clone();
    app.connect_activate(move |_| {
        // A toggle can arrive after pointer capture but just before GTK starts
        // dispatching. Do not briefly show a window in that race.
        if is_close_requested() {
            app_clone.quit();
            return;
        }

        let ui = create_layer_shell_window(&app_clone, location.clone(), config.clone());
        let window = ui.window.clone();

        // Store the window in our thread-local storage
        OVERLAY_WINDOW.with(|w| {
            *w.borrow_mut() = Some(window.clone());
        });

        OVERLAY_APP.with(|a| {
            *a.borrow_mut() = Some(app_clone.clone());
        });

        window.present();
        if resident {
            RESIDENT.with(|slot| {
                *slot.borrow_mut() = Some(ResidentOverlay {
                    ui,
                    _hold: app_clone.hold(),
                });
            });
        }

        debug!("Libadwaita overlay window created");
    });

    // Keep listening while GTK owns the main loop. A new cursor-clip-toggle process
    // connects here: a resident overlay toggles its visibility, otherwise the
    // overlay quits instead of queueing another. The wait happens on a helper
    // thread so an idle overlay never wakes up.
    std::thread::spawn(move || {
        loop {
            let mut poll_fd = libc::pollfd {
                fd: toggle_listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll_fd is a single initialized pollfd that outlives the call.
            if unsafe { libc::poll(&mut poll_fd, 1, -1) } < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            let requests = match ToggleServer::count_toggle_requests_from(&toggle_listener) {
                Ok(requests) => requests,
                Err(error) => {
                    warn!("Failed to receive overlay toggle request: {error}");
                    usize::from(!resident)
                }
            };
            if requests == 0 {
                continue;
            }
            if !resident {
                gtk4::glib::MainContext::default().invoke(request_quit);
                break;
            }
            // Every request flips visibility; two quick presses cancel out.
            if requests % 2 == 1 {
                gtk4::glib::MainContext::default().invoke(toggle_resident_overlay);
            }
        }
    });

    // Run the application
    app.run_with_args::<String>(&[]);

    // Belt-and-suspenders: clear TLS after run returns
    RESIDENT.with(|slot| {
        slot.borrow_mut().take();
    });
    OVERLAY_WINDOW.with(|w| {
        *w.borrow_mut() = None;
    });
    OVERLAY_APP.with(|a| {
        *a.borrow_mut() = None;
    });
    Ok(())
}

fn configure_color_scheme() {
    let style_manager = adw::StyleManager::default();
    style_manager.set_color_scheme(adw::ColorScheme::Default);
}

/// Create and configure the sync layer shell window
fn create_layer_shell_window(
    app: &Application,
    location: OverlayLocation,
    config: UserConfig,
) -> OverlayUi {
    // Create the main window using Adwaita ApplicationWindow
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Clipboard History")
        .decorated(false)
        .build();
    window.add_css_class("cursor-clip-backdrop");

    // Initialize layer shell for this window
    window.init_layer_shell();

    // Configure layer shell properties
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("cursor-clip"));

    // Cover the output so a click outside the card can close the overlay.
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    if let Some(gdk_monitor) = location.monitor.as_deref().and_then(find_gdk_monitor) {
        window.set_monitor(Some(&gdk_monitor));
    }

    window.set_exclusive_zone(-1);

    // Make window keyboard interactive
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

    // Apply custom styling
    apply_custom_styling(&window);

    // Create and set content (also obtain list_box for navigation)
    let placement = Rc::new(Cell::new(CardPlacement {
        position: config.position,
        margin: config.margin,
        cursor: location.cursor,
    }));
    let content = generate_overlay_content(config);
    let card = content.overlay.clone();
    card.add_css_class("cursor-clip-card");
    card.set_overflow(gtk4::Overflow::Hidden);

    let backdrop = create_backdrop(&card, placement.clone());
    window.set_content(Some(&backdrop));
    content.list_size.apply(
        location.monitor_width,
        location.monitor_height,
        placement.get(),
    );
    content
        .list_size
        .connect_apply(&config_path(), placement.clone());
    content.search_entry.connect_visible_notify({
        let size = Rc::downgrade(&content.list_size);
        let window = window.downgrade();
        let placement = placement.clone();
        move |_| {
            if let (Some(size), Some(window)) = (size.upgrade(), window.upgrade())
                && window.is_mapped()
            {
                size.apply(window.width(), window.height(), placement.get());
            }
        }
    });
    window.connect_realize({
        let size = content.list_size.clone();
        let placement = placement.clone();
        move |window| {
            if let Some(surface) = window.surface() {
                let size = Rc::downgrade(&size);
                let placement = placement.clone();
                surface.connect_layout(move |_, width, height| {
                    if let Some(size) = size.upgrade() {
                        size.apply(width, height, placement.get());
                    }
                });
            }
        }
    });

    let backdrop_click = gtk4::GestureClick::new();
    backdrop_click.set_button(0);
    backdrop_click.connect_pressed({
        let card_widget: gtk4::Widget = card.clone().upcast();
        move |gesture, _, click_x, click_y| {
            let Some(widget) = gesture.widget() else {
                return;
            };
            // Let the modal dialog handle clicks, including its backdrop.
            if widget
                .downcast_ref::<adw::ApplicationWindow>()
                .is_some_and(|window| window.visible_dialog().is_some())
            {
                return;
            }
            let inside_card = widget
                .pick(click_x, click_y, gtk4::PickFlags::DEFAULT)
                .is_some_and(|picked| picked == card_widget || picked.is_ancestor(&card_widget));
            if !inside_card {
                debug!("Click outside the overlay card - closing");
                request_quit();
            }
        }
    });
    window.add_controller(backdrop_click);

    // Esc/arrows/Enter/Delete handling and type-to-search
    let key_controller = generate_key_controller(
        &content.list_box,
        &content.history_state,
        &content.search_entry,
    );
    window.add_controller(key_controller);

    // Start in the history list; typing reveals and focuses search on demand.
    window.connect_map({
        let controls = content.persistence;
        move |_| {
            open_history(&controls);
        }
    });

    // Add close request handler to ensure any window close goes through our logic
    window.connect_close_request(|_window| {
        debug!("Window close requested - closing overlay and capture layer");
        request_quit();
        // Stop default handler to avoid double-close reentrancy during teardown
        gtk4::glib::Propagation::Stop
    });

    OverlayUi {
        window,
        backdrop,
        list_size: content.list_size,
        list_box: content.list_box,
        history_state: content.history_state,
        search_entry: content.search_entry,
        menu_revealer: content.menu_revealer,
        placement,
    }
}

fn toggle_resident_overlay() {
    let visible = RESIDENT.with(|resident| {
        resident
            .borrow()
            .as_ref()
            .is_some_and(|resident| resident.ui.window.is_visible())
    });
    if visible {
        hide_resident_overlay();
    } else {
        show_resident_overlay();
    }
}

fn hide_resident_overlay() {
    RESIDENT.with(|resident| {
        if let Some(resident) = resident.borrow().as_ref() {
            if let Some(dialog) = resident.ui.window.visible_dialog() {
                dialog.force_close();
            }
            resident.ui.window.set_visible(false);
        }
    });
}

/// Re-open the kept overlay: apply history changes to the existing rows,
/// reset search/menu/scroll and place the card on the current output.
fn show_resident_overlay() {
    RESIDENT.with(|resident| {
        let resident = resident.borrow();
        let Some(resident) = resident.as_ref() else {
            return;
        };
        let ui = &resident.ui;
        let mut placement = ui.placement.get();
        if let Some(config) = overlay_config(&config_path()) {
            ui.list_size.set_requested(config.width, config.height);
            placement.position = config.position;
            placement.margin = config.margin;
        }

        ui.history_state.search_query.borrow_mut().clear();
        ui.search_entry.set_text("");
        ui.search_entry.set_visible(false);
        ui.menu_revealer.set_reveal_child(false);
        ui.menu_revealer.set_visible(false);
        rebuild_list(&ui.list_box, &ui.history_state);
        refresh_time_labels(&ui.list_box, &ui.history_state);
        if let Some(scrolled) = ui
            .list_box
            .ancestor(gtk4::ScrolledWindow::static_type())
            .and_then(|widget| widget.downcast::<gtk4::ScrolledWindow>().ok())
        {
            scrolled.vadjustment().set_value(0.0);
        }

        let mut monitor_width = ui.window.width();
        let mut monitor_height = ui.window.height();
        if let Some(location) = hyprland::overlay_location(placement.position) {
            let monitor = location.monitor.as_deref().and_then(find_gdk_monitor);
            ui.window.set_monitor(monitor.as_ref());
            placement.cursor = location.cursor;
            monitor_width = location.monitor_width;
            monitor_height = location.monitor_height;
        } else if placement.position != PopupPosition::Cursor {
            ui.window.set_monitor(None);
        }

        ui.list_size.apply(monitor_width, monitor_height, placement);
        ui.placement.set(placement);
        ui.backdrop.queue_allocate();
        ui.window.present();
    });
}

fn find_gdk_monitor(connector: &str) -> Option<gtk4::gdk::Monitor> {
    let display = gtk4::gdk::Display::default()?;
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|index| monitors.item(index))
        .filter_map(|item| item.downcast::<gtk4::gdk::Monitor>().ok())
        .find(|monitor| monitor.connector().as_deref() == Some(connector))
}

fn dimension_entry(label: &str, value: i32) -> gtk4::Entry {
    let entry = gtk4::Entry::builder()
        .width_chars(4)
        .max_width_chars(5)
        .placeholder_text(label)
        .input_purpose(gtk4::InputPurpose::Digits)
        .text(value.to_string())
        .build();
    entry.set_tooltip_text(Some(label));
    entry
}

struct ListSize {
    header: adw::HeaderBar,
    scrolled: gtk4::ScrolledWindow,
    width_entry: gtk4::Entry,
    height_entry: gtk4::Entry,
    apply_button: Button,
    error: Label,
    requested: Cell<(i32, i32)>,
    last_width: Cell<Option<i32>>,
}

impl ListSize {
    fn set_requested(&self, width: i32, height: i32) {
        self.requested.set((width, height));
        self.width_entry.set_text(&width.to_string());
        self.height_entry.set_text(&height.to_string());
        self.error.set_visible(false);
    }

    fn apply(&self, area_width: i32, area_height: i32, placement: CardPlacement) {
        let (requested_width, requested_height) = self.requested.get();
        let (header_min, _, _, _) = self.header.measure(Orientation::Horizontal, -1);
        let minimum = header_min.max(OVERLAY_MIN_WIDTH);
        // The card CSS adds a one-pixel border on each side of the list.
        let max_width = if area_width > 0 {
            (placement.available(area_width) - 2).min(1200)
        } else {
            1200
        };
        let width = requested_width.min(max_width).max(minimum);
        if requested_width < minimum
            && (self.last_width.get() != Some(requested_width)
                || self.scrolled.min_content_width() != width)
        {
            eprintln!(
                "cursor-clip: configured width {requested_width} is too small (header minimum: {header_min} logical pixels). Using the minimum viable width of {minimum} logical pixels."
            );
        }
        self.last_width.set(Some(requested_width));
        self.scrolled.set_min_content_width(width);
        // The vertical box also contains the header and optional search field.
        let chrome_height = self.scrolled.parent().map_or(0, |parent| {
            parent.measure(Orientation::Vertical, width).1
                - self.scrolled.measure(Orientation::Vertical, width).1
        });
        let max_height = if area_height > 0 {
            (placement.available(area_height) - chrome_height - 2).max(1)
        } else {
            crate::shared::config::DEFAULT_LIST_HEIGHT
        };
        self.scrolled
            .set_min_content_height(requested_height.clamp(1, max_height));
    }

    fn connect_apply(self: &Rc<Self>, path: &Path, placement: Rc<Cell<CardPlacement>>) {
        for entry in [&self.width_entry, &self.height_entry] {
            let button = self.apply_button.downgrade();
            entry.connect_activate(move |_| {
                if let Some(button) = button.upgrade() {
                    button.emit_clicked();
                }
            });
        }
        let size = Rc::downgrade(self);
        let path = path.to_owned();
        self.apply_button.connect_clicked(move |_| {
            let Some(size) = size.upgrade() else { return };
            let positive =
                |entry: &gtk4::Entry| entry.text().trim().parse::<i32>().ok().filter(|n| *n > 0);
            let (Some(width), Some(height)) =
                (positive(&size.width_entry), positive(&size.height_entry))
            else {
                size.error
                    .set_label("Enter a positive whole number for each dimension.");
                size.error.set_visible(true);
                return;
            };
            if let Err(e) = UserConfig::update(&path, |cfg| {
                cfg.width = width;
                cfg.height = height;
            }) {
                eprintln!(
                    "cursor-clip: could not save size to {}: {e}",
                    path.display()
                );
                size.error
                    .set_label("Could not save the size. Check config.toml.");
                size.error.set_visible(true);
                return;
            }
            size.set_requested(width, height);
            let (width, height) = size
                .scrolled
                .root()
                .and_then(|r| r.downcast::<gtk4::Window>().ok())
                .map_or((0, 0), |window| (window.width(), window.height()));
            size.apply(width, height, placement.get());
        });
    }
}

#[derive(Clone, Copy)]
struct CardPlacement {
    position: PopupPosition,
    margin: i32,
    cursor: (f64, f64),
}

impl CardPlacement {
    fn available(self, extent: i32) -> i32 {
        if self.position == PopupPosition::Center {
            extent
        } else {
            (extent - self.margin.saturating_mul(2)).max(0)
        }
    }

    fn origin(self, free_width: i32, free_height: i32) -> (i32, i32) {
        let left = self.margin.min(free_width);
        let top = self.margin.min(free_height);
        let right = free_width - left;
        let bottom = free_height - top;
        match self.position {
            PopupPosition::Cursor => (
                (self.cursor.0 as i32).clamp(left, right.max(left)),
                (self.cursor.1 as i32).clamp(top, bottom.max(top)),
            ),
            PopupPosition::Center => (free_width / 2, free_height / 2),
            PopupPosition::TopLeft => (left, top),
            PopupPosition::TopRight => (right, top),
            PopupPosition::BottomLeft => (left, bottom),
            PopupPosition::BottomRight => (right, bottom),
        }
    }
}

fn create_backdrop(card: &Overlay, placement: Rc<Cell<CardPlacement>>) -> Overlay {
    let backdrop = Overlay::new();
    // Allocate the card within the output size without adding its position
    // to the window's size request.
    backdrop.add_overlay(card);
    backdrop.connect_get_child_position(move |backdrop, card| {
        let placement = placement.get();
        let fit = |orientation, for_size, extent| {
            let (minimum, natural, _, _) = card.measure(orientation, for_size);
            natural
                .min(placement.available(extent).max(minimum))
                .min(extent)
        };
        let width = fit(Orientation::Horizontal, -1, backdrop.width());
        let height = fit(Orientation::Vertical, width, backdrop.height());
        let (left, top) = placement.origin(backdrop.width() - width, backdrop.height() - height);
        Some(gtk4::gdk::Rectangle::new(left, top, width, height))
    });
    backdrop
}

fn append_settings_toggle(menu: &Box, title: &str, active: bool) -> (Box, CheckButton) {
    let row = Box::new(Orientation::Horizontal, 8);
    row.add_css_class("settings-row");
    let label = Label::new(Some(title));
    label.set_halign(Align::Start);
    label.set_hexpand(true);
    let check = CheckButton::new();
    check.set_active(active);
    label.set_mnemonic_widget(Some(&check));
    row.append(&label);
    row.append(&check);
    menu.append(&row);
    (row, check)
}

fn generate_overlay_content(config: UserConfig) -> OverlayContent {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar
    let header_bar = adw::HeaderBar::new();
    let title = Label::new(Some("Clipboard History"));
    title.set_margin_start(30);
    title.set_margin_end(30);
    header_bar.set_title_widget(Some(&title));
    header_bar.set_centering_policy(adw::CenteringPolicy::Strict);
    // Layer-shell + undecorated windows can render built-in title buttons unreliably.
    // Use an explicit close button styled like a normal Adwaita title button instead.
    header_bar.set_show_end_title_buttons(false);
    header_bar.set_show_start_title_buttons(false);

    let show_trash_default = config.show_trash;
    let show_pin_default = config.show_pin;
    let instant_paste = Rc::new(Cell::new(config.instant_paste));
    let show_trash_state = Rc::new(RefCell::new(show_trash_default));
    let show_pin_state = Rc::new(RefCell::new(show_pin_default));

    // Search and clear are on the left; options and close are on the right.
    let search_button = Button::builder().icon_name("edit-find-symbolic").build();
    search_button.add_css_class("flat");
    search_button.add_css_class("compact-header-action");
    search_button.set_tooltip_text(Some("Search"));

    let three_dot_menu = Button::builder().icon_name("view-more-symbolic").build();
    three_dot_menu.add_css_class("flat");
    three_dot_menu.add_css_class("compact-header-action");
    three_dot_menu.set_tooltip_text(Some("Options"));

    let header_action_group = Box::new(Orientation::Horizontal, 0);
    header_action_group.add_css_class("header-action-group");
    header_action_group.append(&three_dot_menu);

    let close_icon = gtk4::Image::from_icon_name("window-close-symbolic");
    close_icon.set_pixel_size(16);
    close_icon.set_size_request(16, 16);
    close_icon.set_halign(Align::Center);
    close_icon.set_valign(Align::Center);
    close_icon.set_hexpand(true);
    close_icon.set_vexpand(true);

    let close_icon_box = Box::new(Orientation::Horizontal, 0);
    close_icon_box.add_css_class("manual-close-icon");
    close_icon_box.set_size_request(28, 28);
    close_icon_box.set_halign(Align::Center);
    close_icon_box.set_valign(Align::Center);
    close_icon_box.append(&close_icon);

    let close_button = Button::new();
    close_button.set_child(Some(&close_icon_box));
    close_button.add_css_class("flat");
    close_button.add_css_class("manual-close-button");
    close_button.set_size_request(28, 28);
    close_button.set_tooltip_text(Some("Close"));

    let menu_revealer = Revealer::new();
    menu_revealer.set_reveal_child(false);
    menu_revealer.set_visible(false);
    menu_revealer.set_transition_duration(120);
    menu_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
    menu_revealer.set_halign(Align::End);
    menu_revealer.set_valign(Align::Start);
    menu_revealer.set_margin_top(46);
    menu_revealer.set_margin_end(6);
    menu_revealer.add_css_class("menu-revealer");

    let menu_box = Box::new(Orientation::Vertical, 8);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(6);
    menu_box.set_margin_end(6);

    let toggle_row = Box::new(Orientation::Horizontal, 8);
    toggle_row.add_css_class("settings-row");
    let toggle_label = Label::new(Some("Show delete button"));
    toggle_label.set_halign(Align::Start);
    toggle_label.set_hexpand(true);
    let toggle_check = CheckButton::new();
    toggle_check.set_active(show_trash_default);
    toggle_row.append(&toggle_label);
    toggle_row.append(&toggle_check);
    menu_box.append(&toggle_row);

    let pin_toggle_row = Box::new(Orientation::Horizontal, 8);
    pin_toggle_row.add_css_class("settings-row");
    let pin_toggle_label = Label::new(Some("Show pin icon"));
    pin_toggle_label.set_halign(Align::Start);
    pin_toggle_label.set_hexpand(true);
    let pin_toggle_check = CheckButton::new();
    pin_toggle_check.set_active(show_pin_default);
    pin_toggle_row.append(&pin_toggle_label);
    pin_toggle_row.append(&pin_toggle_check);
    menu_box.append(&pin_toggle_row);

    let persistence_toggle_row = Box::new(Orientation::Horizontal, 8);
    persistence_toggle_row.add_css_class("settings-row");
    let persistence_toggle_label = Label::new(Some("Persistent history"));
    persistence_toggle_label.set_halign(Align::Start);
    persistence_toggle_label.set_hexpand(true);
    let persistence_toggle_check = CheckButton::new();
    persistence_toggle_check.set_inconsistent(true);
    persistence_toggle_row.append(&persistence_toggle_label);
    let persistence_spinner = gtk4::Spinner::new();
    persistence_spinner.set_visible(false);
    persistence_toggle_row.append(&persistence_spinner);
    persistence_toggle_row.append(&persistence_toggle_check);
    menu_box.append(&persistence_toggle_row);

    let (_, instant_paste_toggle_check) =
        append_settings_toggle(&menu_box, "Instant paste", instant_paste.get());
    let (type_in_row, type_in_check) =
        append_settings_toggle(&menu_box, "Type in", config.type_instead_of_paste);
    instant_paste_toggle_check
        .bind_property("active", &type_in_row, "sensitive")
        .sync_create()
        .build();

    let size_row = Box::new(Orientation::Horizontal, 6);
    let width_entry = dimension_entry("Width", config.width);
    let height_entry = dimension_entry("Height", config.height);
    let apply_button = Button::with_label("Apply");
    size_row.append(&width_entry);
    size_row.append(&Label::new(Some("×")));
    size_row.append(&height_entry);
    size_row.append(&apply_button);
    menu_box.append(&size_row);
    let size_error = Label::new(None);
    size_error.add_css_class("error");
    size_error.set_halign(Align::Start);
    size_error.set_wrap(true);
    size_error.set_max_width_chars(26);
    size_error.set_visible(false);
    menu_box.append(&size_error);
    let persistence_status = Label::new(None);
    persistence_status.set_halign(Align::Start);
    persistence_status.set_wrap(true);
    persistence_status.set_max_width_chars(26);
    persistence_status.set_visible(false);
    menu_box.append(&persistence_status);
    let history_status = Label::new(None);
    history_status.set_wrap(true);
    history_status.set_max_width_chars(26);
    history_status.set_visible(false);
    menu_box.append(&history_status);

    let menu_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .propagate_natural_height(true)
        .child(&menu_box)
        .build();
    menu_revealer.set_child(Some(&menu_scroll));
    header_bar.pack_end(&close_button);
    header_bar.pack_end(&header_action_group);

    close_button.connect_clicked(move |_| {
        request_quit();
    });

    let clear_button = Button::builder().icon_name("user-trash-symbolic").build();
    clear_button.add_css_class("flat");
    clear_button.add_css_class("compact-header-action");
    clear_button.set_tooltip_text(Some("Clear entire clipboard history"));
    header_bar.pack_start(&clear_button);
    header_bar.pack_start(&search_button);

    main_box.append(&header_bar);

    // Search stays hidden until the search button is clicked or typing starts.
    let search_entry = SearchEntry::new();
    search_entry.set_visible(false);
    search_entry.set_placeholder_text(Some("Type to search"));
    search_entry.set_margin_start(12);
    search_entry.set_margin_end(12);
    search_entry.set_margin_bottom(6);
    search_entry.add_css_class("clipboard-search");
    // Filtering ~100 rows is cheap; react to every keystroke immediately.
    search_entry.set_property("search-delay", 0u32);
    main_box.append(&search_entry);

    let search_entry_for_button = search_entry.clone();
    search_button.connect_clicked(move |_| {
        focus_search_entry(&search_entry_for_button);
        search_entry_for_button.select_region(0, -1);
    });

    // Create scrolled window for the clipboard list
    let scrolled_window = gtk4::ScrolledWindow::new();
    // External hides the horizontal scrollbar without letting row widths size the card.
    scrolled_window.set_policy(gtk4::PolicyType::External, gtk4::PolicyType::Automatic);

    // Create list box for clipboard items
    let list_box = gtk4::ListBox::new();
    // Use custom styling instead of the default boxed-list to create floating cards
    list_box.add_css_class("clipboard-list");
    //list_box.set_margin_top(6);
    list_box.set_margin_bottom(6);
    list_box.set_margin_start(4);
    list_box.set_margin_end(4);
    list_box.set_selection_mode(gtk4::SelectionMode::Single);

    list_box.set_focusable(true);

    let placeholder = Label::new(None);
    placeholder.add_css_class("dim-label");
    placeholder.set_margin_top(20);
    placeholder.set_margin_bottom(20);
    list_box.set_placeholder(Some(&placeholder));

    let history_state = HistoryListState {
        all_items: Rc::new(RefCell::new(Vec::new())),
        search_query: Rc::new(RefCell::new(String::new())),
        show_trash: show_trash_state,
        show_pin: show_pin_state,
        render_generation: Rc::new(Cell::new(0)),
        current_snapshot: Rc::new(Cell::new(false)),
        placeholder,
    };

    let history_state_for_filter = history_state.clone();
    list_box.set_filter_func(move |row| {
        let query = history_state_for_filter
            .search_query
            .borrow()
            .trim()
            .to_lowercase();
        if query.is_empty() {
            return true;
        }
        row_item_id(row).is_none_or(|item_id| {
            history_state_for_filter
                .all_items
                .borrow()
                .iter()
                .find(|item| item.item_id == item_id)
                .is_some_and(|item| item_matches_query(item, &query))
        })
    });

    rebuild_list(&list_box, &history_state);
    select_first_row(&list_box);

    // Handle item activation (Enter/Space/double-click) instead of mere selection
    let history_state_for_activation = history_state.clone();
    let instant_paste_for_activation = instant_paste.clone();
    list_box.connect_row_activated(move |_, row| {
        if !history_state_for_activation.can_act_on_rows() {
            return;
        }
        let Some(item_id) = row_item_id(row) else {
            return;
        };
        let items = history_state_for_activation.all_items.borrow();
        if let Some(item) = items.iter().find(|item| item.item_id == item_id) {
            let instant_paste = instant_paste_for_activation.get();
            debug!(
                "Activated clipboard item ID {}: {}",
                item.item_id, item.content_preview
            );

            match FrontendClient::new() {
                Ok(mut client) => {
                    if let Err(e) = client.set_clipboard_by_id(item.item_id, instant_paste) {
                        error!("Error setting clipboard by ID: {}", e);
                    } else {
                        info!("Clipboard set by ID: {}", item.item_id);
                        request_quit();
                    }
                }
                Err(e) => {
                    error!("Error creating frontend client: {}", e);
                }
            }
        }
    });

    scrolled_window.set_child(Some(&list_box));
    main_box.append(&scrolled_window);

    set_delete_buttons_visible(&list_box, show_trash_default);
    set_pin_icons_visible(&list_box, show_pin_default);

    let list_box_for_toggle = list_box.clone();
    let history_state_for_toggle = history_state.clone();
    toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        if let Err(e) = UserConfig::update(&config_path(), |config| config.show_trash = state) {
            warn!("Failed to save config: {e}");
        }
        *history_state_for_toggle.show_trash.borrow_mut() = state;
        set_delete_buttons_visible(&list_box_for_toggle, state);
    });

    let list_box_for_pin_toggle = list_box.clone();
    let history_state_for_pin_toggle = history_state.clone();
    pin_toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        if let Err(e) = UserConfig::update(&config_path(), |config| config.show_pin = state) {
            warn!("Failed to save config: {e}");
        }
        *history_state_for_pin_toggle.show_pin.borrow_mut() = state;
        set_pin_icons_visible(&list_box_for_pin_toggle, state);
    });

    let persistence_controls = Rc::new(PersistenceControls {
        check: persistence_toggle_check.clone(),
        spinner: persistence_spinner,
        status: persistence_status,
        list_box: list_box.clone(),
        clear_button: clear_button.clone(),
        history_state: history_state.clone(),
        history_status,
        refresh_generation: Cell::new(0),
        confirmed: Cell::new(None),
        checking: Cell::new(false),
    });
    let controls_for_persistence_toggle = Rc::downgrade(&persistence_controls);
    persistence_toggle_check.connect_toggled(move |check| {
        if check.is_sensitive()
            && let Some(controls) = controls_for_persistence_toggle.upgrade()
        {
            update_persistence(controls, Some(check.is_active()));
        }
    });

    instant_paste_toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        instant_paste.set(state);
        if let Err(e) = UserConfig::update(&config_path(), |config| config.instant_paste = state) {
            warn!("Failed to save config: {}", e);
        }
    });

    let reverting = Cell::new(false);
    let type_in_changed = type_in_check.connect_toggled(move |check| {
        if reverting.get() {
            return;
        }
        let active = check.is_active();
        if let Err(e) = UserConfig::update(&config_path(), |cfg| cfg.type_instead_of_paste = active)
        {
            warn!("Failed to save Type in setting: {e}");
            reverting.set(true);
            check.set_active(!active);
            reverting.set(false);
        }
    });
    // A resident overlay can outlive edits made directly to config.toml.
    menu_box.connect_map(move |_| {
        if let Ok(config) = UserConfig::read(&config_path()) {
            type_in_check.block_signal(&type_in_changed);
            type_in_check.set_active(config.type_instead_of_paste);
            type_in_check.unblock_signal(&type_in_changed);
        }
    });

    let list_box_for_search = list_box.clone();
    let history_state_for_search = history_state.clone();
    search_entry.connect_search_changed(move |entry| {
        *history_state_for_search.search_query.borrow_mut() = entry.text().to_string();
        apply_search_filter(&list_box_for_search, &history_state_for_search);
    });

    let list_box_for_search_activate = list_box.clone();
    search_entry.connect_activate(move |_| {
        if let Some(row) = list_box_for_search_activate.selected_row() {
            row.emit_by_name::<()>("activate", &[]);
        }
    });

    let search_key_controller = gtk4::EventControllerKey::new();
    let list_box_for_search_keys = list_box.clone();
    let search_entry_for_search_keys = search_entry.clone();
    search_key_controller.connect_key_pressed(move |_, key, _, state| {
        use gtk4::gdk::{Key, ModifierType};

        match key {
            Key::u | Key::U
                if state.contains(ModifierType::CONTROL_MASK)
                    && search_entry_for_search_keys.position() > 0 =>
            {
                let cursor = search_entry_for_search_keys.position();
                search_entry_for_search_keys.delete_text(0, cursor);
                search_entry_for_search_keys.set_position(0);
                gtk4::glib::Propagation::Stop
            }
            Key::Down => {
                if select_next_row(&list_box_for_search_keys, true) {
                    gtk4::glib::Propagation::Stop
                } else {
                    gtk4::glib::Propagation::Proceed
                }
            }
            Key::Up => {
                if select_previous_row(&list_box_for_search_keys, true) {
                    gtk4::glib::Propagation::Stop
                } else {
                    gtk4::glib::Propagation::Proceed
                }
            }
            _ => gtk4::glib::Propagation::Proceed,
        }
    });
    search_entry.add_controller(search_key_controller);

    let menu_revealer_toggle = menu_revealer.clone();
    three_dot_menu.connect_clicked(move |_| {
        let next_state = !menu_revealer_toggle.is_child_revealed();
        menu_revealer_toggle.set_visible(next_state);
        menu_revealer_toggle.set_reveal_child(next_state);
    });

    let history_for_clear = history_state.clone();
    clear_button.connect_clicked(move |button| {
        if !history_for_clear.can_act_on_rows() {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Clear clipboard history?"),
            Some("All clipboard history items will be deleted. This cannot be undone."),
        );
        dialog.add_response("cancel", "_Cancel");
        dialog.add_response("clear", "_Clear");
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        let history_for_response = history_for_clear.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "clear" || !history_for_response.can_act_on_rows() {
                return;
            }
            match FrontendClient::new() {
                Ok(mut client) => {
                    if let Err(e) = client.clear_history() {
                        error!("Error clearing clipboard history: {}", e);
                    } else {
                        info!("Clipboard history cleared");
                        // Close the overlay after clearing.
                        request_quit();
                    }
                }
                Err(e) => {
                    error!("Error creating frontend client: {}", e);
                }
            }
        });
        dialog.present(Some(button));
    });

    let overlay = Overlay::new();
    overlay.set_child(Some(&main_box));
    overlay.add_overlay(&menu_revealer);
    overlay.set_measure_overlay(&menu_revealer, true);

    OverlayContent {
        persistence: persistence_controls,
        overlay,
        list_size: Rc::new(ListSize {
            header: header_bar,
            scrolled: scrolled_window,
            width_entry,
            height_entry,
            apply_button,
            error: size_error,
            requested: Cell::new((config.width, config.height)),
            last_width: Cell::new(None),
        }),
        list_box,
        history_state,
        search_entry,
        menu_revealer,
    }
}

/// Build the key controller: Esc hides and clears search or closes, arrows/Enter
/// navigate and activate, Delete removes and Ctrl+P pins the selected item.
/// Any other typing goes to the search field, wherever the focus is.
fn generate_key_controller(
    list_box: &gtk4::ListBox,
    history_state: &HistoryListState,
    search_entry: &SearchEntry,
) -> gtk4::EventControllerKey {
    let controller = gtk4::EventControllerKey::new();
    // Run before the focused widget so Esc and type-to-search always work.
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let list_box_for_keys = list_box.clone();
    let history_state_for_keys = history_state.clone();
    let search_entry_for_keys = search_entry.clone();
    controller.connect_key_pressed(move |controller, key, _, state| {
        use gtk4::gdk::{Key, ModifierType};

        // Modal dialogs own keyboard input; don't search, activate or close history.
        if controller
            .widget()
            .and_then(|widget| widget.downcast::<adw::ApplicationWindow>().ok())
            .is_some_and(|window| window.visible_dialog().is_some())
        {
            return gtk4::glib::Propagation::Proceed;
        }

        if key == Key::Escape {
            if search_entry_for_keys.is_visible() {
                search_entry_for_keys.set_text("");
                search_entry_for_keys.set_visible(false);
                select_first_row(&list_box_for_keys);
            } else {
                request_quit();
            }
            return gtk4::glib::Propagation::Stop;
        }

        // Dimension editors own typing, navigation and Enter while focused.
        let focused = controller
            .widget()
            .and_then(|w| w.root())
            .and_then(|root| root.focus());
        if focused.as_ref().is_some_and(|focus| {
            focus.ancestor(gtk4::Entry::static_type()).is_some() || focus.is::<gtk4::Entry>()
        }) {
            return gtk4::glib::Propagation::Proceed;
        }

        // Letters now go to the search field, so vim-style navigation moves
        // to Ctrl+J/Ctrl+K and works from the search field as well.
        if state.contains(ModifierType::CONTROL_MASK) && matches!(key, Key::j | Key::J) {
            select_next_row(&list_box_for_keys, false);
            return gtk4::glib::Propagation::Stop;
        }
        if state.contains(ModifierType::CONTROL_MASK) && matches!(key, Key::k | Key::K) {
            if !select_previous_row(&list_box_for_keys, false) && search_entry_for_keys.is_visible()
            {
                focus_search_entry(&search_entry_for_keys);
            }
            return gtk4::glib::Propagation::Stop;
        }

        // The search field handles its own editing, Up/Down and Enter.
        if has_focus_within(&search_entry_for_keys) {
            return gtk4::glib::Propagation::Proceed;
        }

        // Row shortcuts apply when a row itself is focused, not one of its
        // inline delete/pin buttons (those keep Enter/Space for activation).
        let focus_on_row = has_focus_within(&list_box_for_keys)
            && focused.is_none_or(|widget| {
                widget.is::<gtk4::ListBoxRow>() || widget.is::<gtk4::ListBox>()
            });
        let has_shortcut_modifier = state.intersects(
            ModifierType::CONTROL_MASK | ModifierType::ALT_MASK | ModifierType::SUPER_MASK,
        );

        match key {
            Key::Delete | Key::KP_Delete if focus_on_row => {
                if let Some(id) = selected_item_id(&list_box_for_keys) {
                    delete_item(&list_box_for_keys, &history_state_for_keys, id);
                }
                gtk4::glib::Propagation::Stop
            }
            Key::p | Key::P if focus_on_row && state.contains(ModifierType::CONTROL_MASK) => {
                if let Some(id) = selected_item_id(&list_box_for_keys) {
                    toggle_item_pin(&list_box_for_keys, &history_state_for_keys, id);
                }
                gtk4::glib::Propagation::Stop
            }
            Key::Up | Key::KP_Up if focus_on_row => {
                if !select_previous_row(&list_box_for_keys, false)
                    && search_entry_for_keys.is_visible()
                {
                    focus_search_entry(&search_entry_for_keys);
                }
                gtk4::glib::Propagation::Stop
            }
            Key::Down | Key::KP_Down if focus_on_row => {
                select_next_row(&list_box_for_keys, false);
                gtk4::glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter if focus_on_row => {
                if let Some(row) = list_box_for_keys.selected_row() {
                    row.emit_by_name::<()>("activate", &[]);
                }
                gtk4::glib::Propagation::Stop
            }
            // Space keeps activating focused buttons and checkboxes.
            Key::space if !focus_on_row => gtk4::glib::Propagation::Proceed,
            _ if !has_shortcut_modifier
                && ((key == Key::BackSpace && search_entry_for_keys.is_visible())
                    || key.to_unicode().is_some_and(|c| !c.is_control())) =>
            {
                focus_search_entry(&search_entry_for_keys);
                if let Some(text) = search_entry_for_keys.delegate() {
                    controller.forward(&text);
                }
                gtk4::glib::Propagation::Stop
            }
            _ => gtk4::glib::Propagation::Proceed,
        }
    });
    controller
}

/// Show and focus the search field without selecting its text, so typing appends.
fn focus_search_entry(search_entry: &SearchEntry) {
    search_entry.set_visible(true);
    match search_entry
        .delegate()
        .and_then(|editable| editable.downcast::<gtk4::Text>().ok())
    {
        Some(text) => {
            text.grab_focus_without_selecting();
        }
        None => {
            search_entry.grab_focus();
        }
    }
}

fn has_focus_within(widget: &impl IsA<gtk4::Widget>) -> bool {
    widget
        .state_flags()
        .contains(gtk4::StateFlags::FOCUS_WITHIN)
}

fn selected_item_id(list_box: &gtk4::ListBox) -> Option<u64> {
    row_item_id(&list_box.selected_row()?)
}

fn delete_item(list_box: &gtk4::ListBox, history_state: &HistoryListState, item_id: u64) {
    if !history_state.can_act_on_rows() {
        return;
    }

    if let Err(error) =
        FrontendClient::new().and_then(|mut client| client.delete_item_by_id(item_id))
    {
        error!("Could not delete clipboard item {item_id}: {error}");
        return;
    }
    history_state
        .all_items
        .borrow_mut()
        .retain(|item| item.item_id != item_id);

    rebuild_list(list_box, history_state);
    select_first_row(list_box);
}

fn toggle_item_pin(list_box: &gtk4::ListBox, history_state: &HistoryListState, item_id: u64) {
    if !history_state.can_act_on_rows() {
        return;
    }
    let Some(pinned) = next_pinned_state(history_state, item_id) else {
        return;
    };

    if let Err(error) =
        FrontendClient::new().and_then(|mut client| client.set_pinned(item_id, pinned))
    {
        error!("Could not update pinned state for clipboard item {item_id}: {error}");
        return;
    }

    apply_pinned_state(history_state, item_id, pinned);
    rebuild_list(list_box, history_state);
    select_row_by_item_id(list_box, item_id);
    debug!("Updated pinned state for clipboard item ID {}", item_id);
}

/// Apply custom CSS styling for modern GNOME-style rounded window
fn apply_custom_styling(window: &adw::ApplicationWindow) {
    let css_provider = gtk4::CssProvider::new();
    let display = gtk4::prelude::WidgetExt::display(window);
    let style_manager = adw::StyleManager::for_display(&display);

    load_overlay_css(&css_provider, style_manager.is_dark());

    {
        let css_provider = css_provider.clone();
        style_manager.connect_dark_notify(move |style_manager| {
            load_overlay_css(&css_provider, style_manager.is_dark());
        });
    }

    gtk4::style_context_add_provider_for_display(
        &display,
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn load_overlay_css(css_provider: &gtk4::CssProvider, is_dark: bool) {
    css_provider.load_from_data(if is_dark {
        "
        window.cursor-clip-backdrop {
            background: transparent;
            border: none;
            box-shadow: none;
        }

        .cursor-clip-card {
            border-radius: 12px;
            background: #222226;
            background: @window_bg_color;
            color: @window_fg_color;
            border: 1px solid alpha(#ffffff, 0.10);
            border: 1px solid alpha(@window_fg_color, 0.10);
            box-shadow: 0 10px 30px alpha(#000000, 0.22);
        }

        headerbar {
            background: transparent;
            box-shadow: none;
        }

        .clipboard-list {
            background: transparent;
        }

        .clipboard-item {
            background: #343437;
            background: @card_bg_color;
            border: 2px solid transparent;
            border-radius: 10px;
            padding: 4px 4px;
            margin: 6px 12px;
            transition: border-color 150ms ease, box-shadow 150ms ease, background 150ms ease;
        }

        .clipboard-item:hover {
            border-color: #3584E4;
            border-color: @accent_bg_color;
            background: shade(#343437, 1.05);
            background: mix(@card_bg_color, @window_fg_color, 0.08);
        }

        .clipboard-item:selected {
            border-color: #3584E4;
            border-color: @accent_bg_color;
            background: alpha(#3584E4, 0.18);
            background: alpha(@accent_bg_color, 0.18);
        }

        .clipboard-preview {
            opacity: 0.9;
            color: @window_fg_color;
        }

        .clipboard-preview.monospace {
            font-family: monospace;
        }

        .clipboard-time {
            font-size: 0.8em;
            opacity: 0.6;
            color: @window_fg_color;
        }

        .clipboard-delete {
            color: #bfc3c7;
            color: alpha(@window_fg_color, 0.75);
            padding: 2px 4px;
        }

        .clipboard-pin {
            color: #bfc3c7;
            color: alpha(@window_fg_color, 0.75);
            padding: 2px 4px;
        }

        .clipboard-item:hover .clipboard-delete,
        .clipboard-delete:hover {
            color: #ffffff;
            color: @window_fg_color;
        }

        .clipboard-item:hover .clipboard-pin {
            color: #ffffff;
            color: @window_fg_color;
        }

        .clipboard-pin:hover {
            color: #ffffff;
            color: @window_fg_color;
        }

        .clipboard-pin.pinned {
            color: #ffffff;
            color: @accent_color;
        }

        .manual-close-button {
            min-width: 28px;
            min-height: 28px;
            padding: 0;
            background: transparent;
            box-shadow: none;
        }

        .manual-close-button:hover,
        .manual-close-button:active {
            background: transparent;
            box-shadow: none;
        }

        .manual-close-icon {
            min-width: 28px;
            min-height: 28px;
            border-radius: 999px;
            background: #343437;
            background: @card_bg_color;
        }

        .manual-close-icon image {
            color: #f4f5f6;
            color: @window_fg_color;
        }

        .manual-close-button:hover .manual-close-icon {
            background: shade(#343437, 1.12);
            background: mix(@card_bg_color, @window_fg_color, 0.12);
        }

        .manual-close-button:hover .manual-close-icon image {
            color: #ffffff;
            color: @window_fg_color;
        }

        .manual-close-button:active .manual-close-icon {
            background: shade(#343437, 0.92);
            background: mix(@card_bg_color, @window_fg_color, 0.04);
        }

        .compact-header-action {
            min-width: 28px;
            min-height: 28px;
            padding-left: 0;
            padding-right: 0;
        }

        .settings-row {
            border-radius: 10px;
            padding: 6px 8px;
            transition: background-color 120ms ease;
        }

        .settings-row:hover,
        .settings-row:focus-within {
            background-color: alpha(@popover_fg_color, 0.08);
        }

        .menu-revealer {
            background: #2b2b2f;
            background: @popover_bg_color;
            border: 1px solid alpha(#ffffff, 0.10);
            border: 1px solid alpha(@popover_fg_color, 0.10);
            border-radius: 8px;
            padding: 6px 8px;
            color: @popover_fg_color;
        }
        "
    } else {
        "
        window.cursor-clip-backdrop {
            background: transparent;
            border: none;
            box-shadow: none;
        }

        .cursor-clip-card {
            border-radius: 12px;
            background: #f6f7f9;
            background: @window_bg_color;
            color: @window_fg_color;
            border: 1px solid alpha(#000000, 0.10);
            border: 1px solid alpha(@window_fg_color, 0.10);
            box-shadow: 0 10px 30px alpha(#000000, 0.12);
        }

        headerbar {
            background: transparent;
            box-shadow: none;
        }

        .clipboard-list {
            background: transparent;
        }

        .clipboard-item {
            background: #ffffff;
            background: @card_bg_color;
            border: 2px solid transparent;
            border-radius: 10px;
            padding: 4px 4px;
            margin: 6px 12px;
            transition: border-color 150ms ease, box-shadow 150ms ease, background 150ms ease;
        }

        .clipboard-item:hover {
            border-color: #1c71d8;
            border-color: @accent_bg_color;
            background: shade(#ffffff, 0.96);
            background: mix(@card_bg_color, @window_fg_color, 0.04);
        }

        .clipboard-item:selected {
            border-color: #1c71d8;
            border-color: @accent_bg_color;
            background: alpha(#1c71d8, 0.12);
            background: alpha(@accent_bg_color, 0.12);
        }

        .clipboard-preview {
            opacity: 0.9;
            color: @window_fg_color;
        }

        .clipboard-preview.monospace {
            font-family: monospace;
        }

        .clipboard-time {
            font-size: 0.8em;
            opacity: 0.6;
            color: @window_fg_color;
        }

        .clipboard-delete {
            color: #5e6268;
            color: alpha(@window_fg_color, 0.7);
            padding: 2px 4px;
        }

        .clipboard-pin {
            color: #5e6268;
            color: alpha(@window_fg_color, 0.7);
            padding: 2px 4px;
        }

        .clipboard-item:hover .clipboard-delete,
        .clipboard-delete:hover {
            color: #1f2328;
            color: @window_fg_color;
        }

        .clipboard-item:hover .clipboard-pin {
            color: #1f2328;
            color: @window_fg_color;
        }

        .clipboard-pin:hover {
            color: #1f2328;
            color: @window_fg_color;
        }

        .clipboard-pin.pinned {
            color: #1f2328;
            color: @accent_color;
        }

        .manual-close-button {
            min-width: 28px;
            min-height: 28px;
            padding: 0;
            background: transparent;
            box-shadow: none;
        }

        .manual-close-button:hover,
        .manual-close-button:active {
            background: transparent;
            box-shadow: none;
        }

        .manual-close-icon {
            min-width: 28px;
            min-height: 28px;
            border-radius: 999px;
            background: #ffffff;
            background: @card_bg_color;
        }

        .manual-close-icon image {
            color: #1f2328;
            color: @window_fg_color;
        }

        .manual-close-button:hover .manual-close-icon {
            background: shade(#ffffff, 0.92);
            background: mix(@card_bg_color, @window_fg_color, 0.08);
        }

        .manual-close-button:hover .manual-close-icon image {
            color: #111318;
            color: @window_fg_color;
        }

        .manual-close-button:active .manual-close-icon {
            background: shade(#ffffff, 0.86);
            background: mix(@card_bg_color, @window_fg_color, 0.12);
        }

        .compact-header-action {
            min-width: 28px;
            min-height: 28px;
            padding-left: 0;
            padding-right: 0;
        }

        .settings-row {
            border-radius: 10px;
            padding: 6px 8px;
            transition: background-color 120ms ease;
        }

        .settings-row:hover,
        .settings-row:focus-within {
            background-color: alpha(@popover_fg_color, 0.08);
        }

        .menu-revealer {
            background: #ffffff;
            background: @popover_bg_color;
            border: 1px solid alpha(#000000, 0.10);
            border: 1px solid alpha(@popover_fg_color, 0.10);
            border-radius: 8px;
            padding: 6px 8px;
            box-shadow: 0 2px 8px alpha(#000000, 0.10);
            color: @popover_fg_color;
        }
        "
    });
}

/// Create a clipboard history item row from backend data
fn generate_listboxrow_from_preview(
    item: &ClipboardItemPreview,
    list_box: &gtk4::ListBox,
    history_state: &HistoryListState,
    show_trash: bool,
    show_pin: bool,
) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.add_css_class("clipboard-item");
    row.set_widget_name(&row_key(item));

    let main_box = Box::new(Orientation::Vertical, 6);
    main_box.set_margin_top(8);
    main_box.set_margin_bottom(8);
    main_box.set_margin_start(12);
    main_box.set_margin_end(12);

    // Header with content type and time
    let header_box = Box::new(Orientation::Horizontal, 8);

    let type_label = Label::new(Some(item.content_type.icon()));
    type_label.add_css_class("caption");

    let type_text = Label::new(Some(item.content_type.as_str()));
    type_text.add_css_class("caption");
    type_text.set_halign(Align::Start);
    type_text.set_hexpand(true);

    let time_label = Label::new(Some(&format_timestamp(item.timestamp)));
    time_label.add_css_class("caption");
    time_label.add_css_class("clipboard-time");
    time_label.set_halign(Align::End);

    let pin_button = Button::builder().icon_name("view-pin-symbolic").build();
    pin_button.add_css_class("flat");
    pin_button.add_css_class("clipboard-pin");
    if item.pinned {
        pin_button.add_css_class("pinned");
        pin_button.set_tooltip_text(Some("Unpin"));
    } else {
        pin_button.set_tooltip_text(Some("Pin"));
    }
    pin_button.set_visible(show_pin);

    let delete_button = Button::builder().icon_name("user-trash-symbolic").build();
    delete_button.add_css_class("flat");
    delete_button.add_css_class("destructive-action");
    delete_button.add_css_class("clipboard-delete");
    delete_button.set_tooltip_text(Some("Delete item"));
    delete_button.set_visible(show_trash);

    header_box.append(&type_label);
    header_box.append(&type_text);
    let action_box = Box::new(Orientation::Horizontal, 0);
    action_box.append(&pin_button);
    action_box.append(&delete_button);

    header_box.append(&time_label);
    header_box.append(&action_box);

    main_box.append(&header_box);

    let rendered_image = item.thumbnail.as_ref().and_then(|bytes| {
        let gbytes = gtk4::glib::Bytes::from(bytes);
        gtk4::gdk::Texture::from_bytes(&gbytes).ok()
    });

    if let Some(texture) = rendered_image {
        let picture = gtk4::Picture::for_paintable(&texture);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_height_request(180);
        picture.set_halign(gtk4::Align::Center);
        picture.add_css_class("clipboard-preview");
        main_box.append(&picture);
    } else {
        // Collapse whitespace runs: Pango cannot wrap inside a long run of spaces, so one
        // indented snippet (e.g. pasted HTML) used to force the whole card to its width.
        let preview: String = item
            .content_preview
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let content_label = Label::new(Some(&preview));
        content_label.add_css_class("clipboard-preview");
        if matches!(
            item.content_type,
            ClipboardContentType::Code | ClipboardContentType::File
        ) {
            content_label.add_css_class("monospace");
        }
        content_label.set_halign(Align::Start);
        content_label.set_wrap(true);
        content_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        content_label.set_max_width_chars(50);
        content_label.set_lines(3);
        content_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        main_box.append(&content_label);
    }

    row.set_child(Some(&main_box));

    let item_id = item.item_id;
    delete_button.connect_clicked({
        let list = list_box.downgrade();
        let state = history_state.clone();
        move |_| {
            if let Some(list) = list.upgrade() {
                delete_item(&list, &state, item_id);
            }
        }
    });
    pin_button.connect_clicked({
        let list = list_box.downgrade();
        let state = history_state.clone();
        move |_| {
            if let Some(list) = list.upgrade() {
                toggle_item_pin(&list, &state, item_id);
            }
        }
    });
    row
}

/// Rows built before the first frame; enough to fill the visible part of the list.
const INITIAL_ROWS: usize = 8;
/// Rows added per frame afterwards.
const ROWS_PER_FRAME: usize = 12;

/// Bring the rows in line with `all_items`. Existing rows are reused and only
/// new items get widgets; the first few are built now and the rest follow
/// frame by frame so a cold start does not lay out every row before showing.
fn rebuild_list(list_box: &gtk4::ListBox, history_state: &HistoryListState) {
    let generation = history_state.render_generation.get().wrapping_add(1);
    history_state.render_generation.set(generation);

    // Also remove expired rows while the overlay stays open or resident.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    history_state
        .all_items
        .borrow_mut()
        .retain(|item| item.expires_at.is_none_or(|expires| expires > now));
    if let Some(deadline) = history_state
        .all_items
        .borrow()
        .iter()
        .filter_map(|item| item.expires_at)
        .min()
    {
        let list_box = list_box.downgrade();
        let history_state = history_state.clone();
        gtk4::glib::timeout_add_local_once(
            std::time::Duration::from_secs(deadline.saturating_sub(now)),
            move || {
                if history_state.render_generation.get() == generation
                    && let Some(list_box) = list_box.upgrade()
                {
                    rebuild_list(&list_box, &history_state);
                }
            },
        );
    }

    let done = sync_rows(list_box, history_state, INITIAL_ROWS);
    apply_search_filter(list_box, history_state);
    if done {
        return;
    }

    let history_state = history_state.clone();
    list_box.add_tick_callback(move |list_box, _| {
        if history_state.render_generation.get() != generation {
            return gtk4::glib::ControlFlow::Break;
        }
        let done = sync_rows(list_box, &history_state, ROWS_PER_FRAME);
        // A search match may only exist once its row has been created.
        if list_box.selected_row().filter(is_row_shown).is_none() {
            select_first_row_without_focus(list_box);
        }
        if done {
            gtk4::glib::ControlFlow::Break
        } else {
            gtk4::glib::ControlFlow::Continue
        }
    });
}

/// One pass of row reconciliation creating at most `budget` new rows.
/// Returns true once the rows match `all_items` exactly.
fn sync_rows(list_box: &gtk4::ListBox, history_state: &HistoryListState, budget: usize) -> bool {
    let items = history_state.all_items.borrow().clone();
    let wanted: std::collections::HashSet<String> = items.iter().map(row_key).collect();

    let mut existing = std::collections::HashMap::new();
    for row in list_rows(list_box) {
        let key = row.widget_name().to_string();
        if wanted.contains(&key) && !existing.contains_key(&key) {
            existing.insert(key, row);
        } else {
            list_box.remove(&row);
        }
    }

    let show_trash = *history_state.show_trash.borrow();
    let show_pin = *history_state.show_pin.borrow();
    let mut created = 0;
    for (position, item) in items.iter().enumerate() {
        let key = row_key(item);
        let index = position as i32;
        if list_box
            .row_at_index(index)
            .is_some_and(|row| row.widget_name() == key)
        {
            continue;
        }
        if let Some(row) = existing.get(&key) {
            list_box.remove(row);
            list_box.insert(row, index);
            continue;
        }
        if created == budget {
            return false;
        }
        let row =
            generate_listboxrow_from_preview(item, list_box, history_state, show_trash, show_pin);
        list_box.insert(&row, index);
        created += 1;
    }
    true
}

fn apply_search_filter(list_box: &gtk4::ListBox, history_state: &HistoryListState) {
    list_box.invalidate_filter();
    history_state
        .placeholder
        .set_label(if history_state.all_items.borrow().is_empty() {
            "No clipboard history yet"
        } else {
            "No matches found"
        });
    select_first_row_without_focus(list_box);
}

/// Row widget name: item id, pinned state and a hash of what the row shows.
/// A pin change rebuilds the row, and so does an id that the daemon reused
/// for different content (ids restart when history is not persistent).
fn row_key(item: &ClipboardItemPreview) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    item.timestamp.hash(&mut hasher);
    item.content_preview.hash(&mut hasher);
    item.thumbnail.as_deref().hash(&mut hasher);
    format!(
        "{}:{}:{:016x}",
        item.item_id,
        u8::from(item.pinned),
        hasher.finish()
    )
}

fn row_item_id(row: &gtk4::ListBoxRow) -> Option<u64> {
    row.widget_name().split(':').next()?.parse().ok()
}

fn list_rows(list_box: &gtk4::ListBox) -> Vec<gtk4::ListBoxRow> {
    let mut rows = Vec::new();
    let mut child = list_box.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk4::ListBoxRow>() {
            rows.push(row);
        }
    }
    rows
}

/// Rows hidden by the search filter are skipped by keyboard navigation.
fn is_row_shown(row: &gtk4::ListBoxRow) -> bool {
    row.is_child_visible() && row.is_selectable()
}

/// Relative times ("5 minutes ago") of reused rows go stale while hidden.
fn refresh_time_labels(list_box: &gtk4::ListBox, history_state: &HistoryListState) {
    let items = history_state.all_items.borrow();
    for row in list_rows(list_box) {
        let Some(item) = row_item_id(&row).and_then(|id| items.iter().find(|i| i.item_id == id))
        else {
            continue;
        };
        if let Some(label) = find_descendant_with_class(row.upcast_ref(), "clipboard-time")
            .and_then(|widget| widget.downcast::<Label>().ok())
        {
            label.set_label(&format_timestamp(item.timestamp));
        }
    }
}

fn find_descendant_with_class(widget: &gtk4::Widget, class_name: &str) -> Option<gtk4::Widget> {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.has_css_class(class_name) {
            return Some(current);
        }
        if let Some(found) = find_descendant_with_class(&current, class_name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

fn item_matches_query(item: &ClipboardItemPreview, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    item.content_preview.to_lowercase().contains(query)
        || item.content_type.as_str().to_lowercase().contains(query)
}

fn select_first_row(list_box: &gtk4::ListBox) {
    select_first_row_with_focus(list_box, true);
}

fn select_first_row_without_focus(list_box: &gtk4::ListBox) {
    select_first_row_with_focus(list_box, false);
}

fn select_first_row_with_focus(list_box: &gtk4::ListBox, grab_focus: bool) -> bool {
    let Some(row) = list_rows(list_box).into_iter().find(is_row_shown) else {
        list_box.unselect_all();
        return false;
    };
    list_box.select_row(Some(&row));
    if grab_focus {
        row.grab_focus();
    }
    true
}

/// Next shown row after (`step` = 1) or before (`step` = -1) `index`.
fn shown_row_from(list_box: &gtk4::ListBox, index: i32, step: i32) -> Option<gtk4::ListBoxRow> {
    let mut index = index + step;
    while let Some(row) = list_box.row_at_index(index) {
        if is_row_shown(&row) {
            return Some(row);
        }
        index += step;
    }
    None
}

fn select_adjacent_row(list_box: &gtk4::ListBox, step: i32, wrap_to_first: bool) -> bool {
    let Some(current) = list_box.selected_row().filter(is_row_shown) else {
        return select_first_row_with_focus(list_box, true);
    };
    if let Some(row) = shown_row_from(list_box, current.index(), step) {
        list_box.select_row(Some(&row));
        row.grab_focus();
        return true;
    }
    wrap_to_first && select_first_row_with_focus(list_box, true)
}

fn select_next_row(list_box: &gtk4::ListBox, wrap_to_first: bool) -> bool {
    select_adjacent_row(list_box, 1, wrap_to_first)
}

fn select_previous_row(list_box: &gtk4::ListBox, wrap_to_first: bool) -> bool {
    select_adjacent_row(list_box, -1, wrap_to_first)
}

fn select_row_by_item_id(list_box: &gtk4::ListBox, item_id: u64) {
    match list_rows(list_box)
        .into_iter()
        .find(|row| row_item_id(row) == Some(item_id) && is_row_shown(row))
    {
        Some(row) => {
            list_box.select_row(Some(&row));
            row.grab_focus();
        }
        None => {
            select_first_row(list_box);
        }
    }
}

fn next_pinned_state(history_state: &HistoryListState, item_id: u64) -> Option<bool> {
    history_state
        .all_items
        .borrow()
        .iter()
        .find(|item| item.item_id == item_id)
        .map(|item| !item.pinned)
}

fn apply_pinned_state(history_state: &HistoryListState, item_id: u64, pinned: bool) {
    let mut items = history_state.all_items.borrow_mut();
    let Some(index) = items.iter().position(|entry| entry.item_id == item_id) else {
        return;
    };

    let mut item = items.remove(index);
    item.pinned = pinned;
    let insert_index = if pinned {
        0
    } else {
        items
            .iter()
            .position(|existing| !existing.pinned)
            .unwrap_or(items.len())
    };
    items.insert(insert_index, item);
}

fn set_delete_buttons_visible(list_box: &gtk4::ListBox, visible: bool) {
    let mut child = list_box.first_child();
    while let Some(widget) = child {
        if let Ok(row) = widget.clone().downcast::<gtk4::ListBoxRow>()
            && let Some(delete_button) = find_button_in_row(&row, "clipboard-delete")
        {
            delete_button.set_visible(visible);
        }
        child = widget.next_sibling();
    }
}

fn set_pin_icons_visible(list_box: &gtk4::ListBox, visible: bool) {
    let mut child = list_box.first_child();
    while let Some(widget) = child {
        if let Ok(row) = widget.clone().downcast::<gtk4::ListBoxRow>()
            && let Some(pin_button) = find_button_in_row(&row, "clipboard-pin")
        {
            pin_button.set_visible(visible);
        }
        child = widget.next_sibling();
    }
}
fn find_button_in_row(row: &gtk4::ListBoxRow, class_name: &str) -> Option<gtk4::Button> {
    let main_box = row.child()?.downcast::<gtk4::Box>().ok()?;
    let header_box = main_box.first_child()?.downcast::<gtk4::Box>().ok()?;
    let mut child = header_box.first_child();
    while let Some(widget) = child {
        if widget.has_css_class(class_name) {
            return widget.downcast::<gtk4::Button>().ok();
        }
        if let Ok(container) = widget.clone().downcast::<gtk4::Box>() {
            let mut inner = container.first_child();
            while let Some(inner_widget) = inner {
                if inner_widget.has_css_class(class_name) {
                    return inner_widget.downcast::<gtk4::Button>().ok();
                }
                inner = inner_widget.next_sibling();
            }
        }
        child = widget.next_sibling();
    }
    None
}

/// Format Unix timestamp to relative time string
fn format_timestamp(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let diff = now.saturating_sub(timestamp);

    if diff < 30 {
        "Just now".to_string()
    } else if diff < 3600 {
        let minutes = diff / 60;
        format!(
            "{} minute{} ago",
            minutes,
            if minutes == 1 { "" } else { "s" }
        )
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else {
        let days = diff / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;
    use crate::shared::config::{DEFAULT_LIST_HEIGHT, DEFAULT_LIST_WIDTH, DEFAULT_MARGIN};
    use std::fs;

    #[test]
    #[ignore = "requires a GTK display"]
    fn type_in_toggle_persists_and_follows_instant_paste() {
        fn check_for(widget: &gtk4::Widget, title: &str) -> Option<CheckButton> {
            if let Some(label) = widget.downcast_ref::<Label>()
                && label.text() == title
            {
                return label.mnemonic_widget()?.downcast().ok();
            }
            std::iter::successors(widget.first_child(), |child| child.next_sibling())
                .find_map(|child| check_for(&child, title))
        }
        crate::test_support::isolated_test(|| {
            adw::init().unwrap();
            let path = config_path();
            UserConfig::update(&path, |cfg| cfg.type_instead_of_paste = true).unwrap();
            let content = generate_overlay_content(UserConfig::read(&path).unwrap());
            let menu = content.menu_revealer.upcast_ref();
            let type_in = check_for(menu, "Type in").unwrap();
            let instant = check_for(menu, "Instant paste").unwrap();
            assert!(type_in.is_active());
            instant.set_active(false);
            assert!(!type_in.is_sensitive());
            instant.set_active(true);
            assert!(type_in.is_sensitive());
            for active in [false, true] {
                type_in.emit_activate();
                assert_eq!(
                    UserConfig::read(&path).unwrap().type_instead_of_paste,
                    active
                );
            }
        });
    }

    #[test]
    fn malformed_config_survives_loading_and_preference_changes() {
        let dir =
            std::env::temp_dir().join(format!("cursor-clip-config-{:x}", rand::random::<u64>()));
        let path = dir.join("config.toml");
        assert_eq!(
            UserConfig::load_or_create(&path).unwrap().width,
            DEFAULT_LIST_WIDTH
        );
        for invalid in [
            "width = [",
            "height = 0",
            "height = \"400\"",
            "persistence_enabled = true\nwidth = \"700\"\n",
            "widht = 700\n",
            "position = \"side\"",
            "margin = -1",
        ] {
            fs::write(&path, invalid).unwrap();
            assert!(overlay_config(&path).is_none());
            assert!(UserConfig::update(&path, |cfg| cfg.show_pin = false).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        fs::write(&path, "width = \"bad\"\npersistent_history = true\n").unwrap();
        assert!(crate::shared::config::read_persistence_enabled(&path).unwrap());
        fs::write(&path, "width = 700\npersistent_history = true\n").unwrap();
        UserConfig::update(&path, |cfg| cfg.show_pin = false).unwrap();
        let config = UserConfig::load_or_create(&path).unwrap();
        assert!(config.persistence_enabled && !config.show_pin);
        assert!(!config.type_instead_of_paste);
        assert_eq!((config.width, config.height), (700, DEFAULT_LIST_HEIGHT));
        assert_eq!(
            (config.position, config.margin),
            (PopupPosition::Cursor, DEFAULT_MARGIN)
        );

        fs::write(
            &path,
            "position = \"bottom-right\"\nmargin = 24\ntype_instead_of_paste = true\n",
        )
        .unwrap();
        UserConfig::update(&path, |cfg| cfg.show_pin = false).unwrap();
        let config = UserConfig::read(&path).unwrap();
        assert!(config.type_instead_of_paste);
        assert_eq!(
            (config.position, config.margin),
            (PopupPosition::BottomRight, 24)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn card_origins_respect_the_output_and_margin() {
        let mut placement = CardPlacement {
            position: PopupPosition::Cursor,
            margin: 24,
            cursor: (125.0, 480.0),
        };
        for (position, origin) in [
            (PopupPosition::Cursor, (125, 326)),
            (PopupPosition::Center, (250, 175)),
            (PopupPosition::TopLeft, (24, 24)),
            (PopupPosition::TopRight, (476, 24)),
            (PopupPosition::BottomLeft, (24, 326)),
            (PopupPosition::BottomRight, (476, 326)),
        ] {
            placement.position = position;
            assert_eq!(placement.origin(500, 350), origin, "{position:?}");
        }
        assert_eq!(placement.origin(12, 0), (0, 0));
        placement.margin = 0;
        assert_eq!(placement.origin(500, 350), (500, 350));
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn oversized_list_fits_the_output_at_the_cursor_and_in_the_center() {
        adw::init().unwrap();
        let config = UserConfig {
            width: 1100,
            height: 900,
            margin: 32,
            ..UserConfig::default()
        };
        let placement = Rc::new(Cell::new(CardPlacement {
            position: config.position,
            margin: config.margin,
            cursor: (798.0, 599.0),
        }));
        let content = generate_overlay_content(config);
        let window = adw::ApplicationWindow::builder().build();
        apply_custom_styling(&window);
        content.overlay.add_css_class("cursor-clip-card");
        let root = create_backdrop(&content.overlay, placement.clone());
        window.set_content(Some(&root));
        content.list_size.apply(0, 0, placement.get());

        for (position, inset) in [(PopupPosition::Cursor, 32), (PopupPosition::Center, 0)] {
            placement.set(CardPlacement {
                position,
                ..placement.get()
            });
            content.list_size.apply(800, 600, placement.get());
            root.allocate(800, 600, -1, None);
            let bounds = content.overlay.compute_bounds(&root).unwrap();
            assert_eq!((bounds.x(), bounds.y()), (inset as f32, inset as f32));
            assert_eq!(bounds.width(), (800 - 2 * inset) as f32);
            assert_eq!(bounds.height(), (600 - 2 * inset) as f32);
        }

        content.list_size.apply(1920, 1080, placement.get());
        root.allocate(1920, 1080, -1, None);
        assert_eq!(content.list_size.scrolled.width(), 1100);
        assert_eq!(content.list_size.scrolled.height(), 900);
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn size_fields_apply_save_and_reload_both_dimensions() {
        adw::init().unwrap();
        let config = UserConfig::default();
        let dir =
            std::env::temp_dir().join(format!("cursor-clip-size-{:x}", rand::random::<u64>()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let item = ClipboardItemPreview {
            item_id: 1,
            content_preview: "long".repeat(300),
            content_type: ClipboardContentType::Text,
            timestamp: 1,
            pinned: false,
            thumbnail: None,
            expires_at: None,
        };
        let window = adw::ApplicationWindow::builder()
            .default_width(1000)
            .default_height(550)
            .build();
        apply_custom_styling(&window);
        let content = generate_overlay_content(config);
        apply_history(&content.list_box, &content.history_state, vec![item]);
        content.overlay.add_css_class("cursor-clip-card");
        let placement = Rc::new(Cell::new(CardPlacement {
            position: PopupPosition::BottomRight,
            margin: 24,
            cursor: (990.0, 540.0),
        }));
        let root = create_backdrop(&content.overlay, placement.clone());
        window.set_content(Some(&root));
        let size = &content.list_size;
        let minimum = size
            .header
            .measure(Orientation::Horizontal, -1)
            .0
            .max(OVERLAY_MIN_WIDTH);
        let wait_for_size = |width, height| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while (size.scrolled.width(), size.scrolled.height()) != (width, height) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "expected {width} × {height}, allocated {} × {}",
                    size.scrolled.width(),
                    size.scrolled.height()
                );
                gtk4::glib::MainContext::default().iteration(false);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let keys = generate_key_controller(
            &content.list_box,
            &content.history_state,
            &content.search_entry,
        );
        window.add_controller(keys.clone());
        size.connect_apply(&path, placement.clone());
        size.apply(1000, 550, placement.get());
        window.present();
        wait_for_size(350, 400);
        let closed_width = content.overlay.width();

        content.persistence.spinner.set_visible(true);
        content
            .persistence
            .status
            .set_label("Updating history storage…");
        content.persistence.status.set_visible(true);
        content
            .persistence
            .history_status
            .set_label("Could not refresh clipboard history. Reopen to retry.");
        content.persistence.history_status.set_visible(true);
        size.height_entry.set_text("");
        size.apply_button.emit_clicked();
        assert!(size.error.get_visible());
        content.menu_revealer.set_visible(true);
        content.menu_revealer.set_reveal_child(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !content.menu_revealer.is_child_revealed() {
            assert!(std::time::Instant::now() < deadline);
            gtk4::glib::MainContext::default().iteration(false);
        }
        assert_eq!(
            content.overlay.width(),
            closed_width,
            "opening settings widened the card"
        );
        assert_eq!(size.scrolled.width(), 350);
        for widget in [
            size.width_entry.upcast_ref::<gtk4::Widget>(),
            size.height_entry.upcast_ref(),
            size.apply_button.upcast_ref(),
        ] {
            let bounds = widget.compute_bounds(&content.overlay).unwrap();
            assert!(
                bounds.width() > 0.0
                    && bounds.x() >= 0.0
                    && bounds.x() + bounds.width() <= closed_width as f32,
                "settings control outside the card: {bounds:?}"
            );
        }
        content.persistence.spinner.set_visible(false);
        content.persistence.status.set_visible(false);
        content.persistence.history_status.set_visible(false);
        assert!(size.width_entry.grab_focus());
        let consumed = keys.emit_by_name::<bool>(
            "key-pressed",
            &[
                &gtk4::gdk::Key::_6,
                &0u32,
                &gtk4::gdk::ModifierType::empty(),
            ],
        );
        assert!(!consumed && !content.search_entry.get_visible());
        size.width_entry.set_text("600");
        size.height_entry.set_text("280");
        size.height_entry.emit_activate();
        wait_for_size(600.max(minimum), 280);
        let title = size
            .header
            .title_widget()
            .unwrap()
            .downcast::<Label>()
            .unwrap();
        assert!(
            title.width()
                >= title
                    .create_pango_layout(Some(&title.text()))
                    .pixel_size()
                    .0,
            "header title was truncated on a wide card: {} pixels",
            title.width()
        );
        let bounds = content.overlay.compute_bounds(&root).unwrap();
        assert_eq!(bounds.x() + bounds.width(), (root.width() - 24) as f32);
        assert_eq!(bounds.y() + bounds.height(), (root.height() - 24) as f32);
        let saved = UserConfig::load_or_create(&path).unwrap();
        assert_eq!((saved.width, saved.height), (600, 280));

        size.height_entry.set_text("");
        size.apply_button.emit_clicked();
        assert!(size.error.get_visible());
        assert_eq!(UserConfig::load_or_create(&path).unwrap().height, 280);

        window.set_visible(false);
        content.menu_revealer.set_visible(false);
        content.search_entry.set_visible(true);
        *content.history_state.search_query.borrow_mut() = "no matches".into();
        apply_search_filter(&content.list_box, &content.history_state);
        UserConfig::update(&path, |cfg| {
            cfg.width = 1;
            cfg.height = 190;
        })
        .unwrap();
        let cfg = UserConfig::load_or_create(&path).unwrap();
        size.set_requested(cfg.width, cfg.height);
        size.apply(1000, 550, placement.get());
        assert_eq!(size.width_entry.text(), "1");
        assert_eq!(size.height_entry.text(), "190");
        window.present();
        wait_for_size(minimum, 190);
        fs::write(&path, "height = [").unwrap();
        size.height_entry.set_text("300");
        size.apply_button.emit_clicked();
        assert!(size.error.get_visible());
        assert_eq!(fs::read_to_string(&path).unwrap(), "height = [");
        window.close();
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::{BackendMessage, FrontendMessage};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    #[track_caller]
    fn until<T>(mut check: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(value) = check() {
                return value;
            }
            assert!(Instant::now() < deadline, "GTK condition timed out");
            gtk4::glib::MainContext::default().iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn reply(mut stream: UnixStream, response: BackendMessage) {
        writeln!(stream, "{}", serde_json::to_string(&response).unwrap()).unwrap();
    }

    fn history() -> BackendMessage {
        BackendMessage::History {
            items: vec![ClipboardItemPreview {
                item_id: 1,
                content_type: ClipboardContentType::Text,
                content_preview: "first".into(),
                timestamp: 0,
                pinned: false,
                expires_at: None,
                thumbnail: None,
            }],
        }
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn row_actions_use_the_target_id_and_wait_for_acknowledgement() {
        crate::test_support::isolated_test(|| {
            adw::init().unwrap();
            let dir = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
                .join("cursor-clip");
            std::fs::create_dir(&dir).unwrap();
            let listener = UnixListener::bind(dir.join("cursor-clip.sock")).unwrap();
            let server = std::thread::spawn(move || {
                for (expected, response) in [
                    (
                        FrontendMessage::SetPinned {
                            id: 2,
                            pinned: true,
                        },
                        BackendMessage::ItemPinned {
                            id: 2,
                            pinned: true,
                        },
                    ),
                    (
                        FrontendMessage::SetPinned {
                            id: 2,
                            pinned: false,
                        },
                        BackendMessage::ItemPinned {
                            id: 2,
                            pinned: false,
                        },
                    ),
                    (
                        FrontendMessage::DeleteItemById { id: 2 },
                        BackendMessage::Error {
                            message: "rejected".into(),
                        },
                    ),
                    (
                        FrontendMessage::DeleteItemById { id: 2 },
                        BackendMessage::ItemDeleted { id: 1 },
                    ),
                    (
                        FrontendMessage::DeleteItemById { id: 2 },
                        BackendMessage::ItemDeleted { id: 2 },
                    ),
                ] {
                    let (stream, _) = listener.accept().unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                    reply(stream, response);
                }
            });

            let content = generate_overlay_content(UserConfig::default());
            let list = &content.list_box;
            let state = &content.history_state;
            let window = adw::ApplicationWindow::builder().build();
            window.set_content(Some(&content.overlay));
            let keys = generate_key_controller(list, state, &content.search_entry);
            window.add_controller(keys.clone());
            window.present();
            let BackendMessage::History { mut items } = history() else {
                unreachable!()
            };
            items.push(ClipboardItemPreview {
                item_id: 2,
                ..items[0].clone()
            });
            apply_history(list, state, items);
            let click = |id, class| {
                let row = list_rows(list)
                    .into_iter()
                    .find(|row| row_item_id(row) == Some(id))
                    .unwrap();
                find_button_in_row(&row, class).unwrap().emit_clicked();
            };
            let press = |key: gtk4::gdk::Key, modifiers: gtk4::gdk::ModifierType| {
                assert!(keys.emit_by_name::<bool>("key-pressed", &[&key, &0u32, &modifiers]));
            };

            // Cached rows are visible but cannot act before the first current snapshot.
            click(2, "clipboard-delete");
            state.current_snapshot.set(true);
            select_first_row(list);
            assert_eq!(selected_item_id(list), Some(1));
            click(2, "clipboard-pin");
            assert!(state.all_items.borrow()[0].pinned);
            assert_eq!(selected_item_id(list), Some(2));
            press(gtk4::gdk::Key::p, gtk4::gdk::ModifierType::CONTROL_MASK);
            assert!(!state.all_items.borrow()[0].pinned);
            press(gtk4::gdk::Key::Delete, gtk4::gdk::ModifierType::empty());
            assert_eq!(state.all_items.borrow().len(), 2);
            select_row_by_item_id(list, 1);
            click(2, "clipboard-delete");
            assert_eq!(state.all_items.borrow().len(), 2);
            click(2, "clipboard-delete");
            assert_eq!(state.all_items.borrow().len(), 1);
            assert_eq!(selected_item_id(list), Some(1));
            window.close();
            server.join().unwrap();
        });
    }

    #[test]
    #[ignore = "requires a GTK display"]
    fn cold_focus_and_persistence_ack_are_independent_of_history() {
        crate::test_support::isolated_test(|| {
            adw::init().unwrap();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let _enter = runtime.enter();
            let dir = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
                .join("cursor-clip");
            std::fs::create_dir(&dir).unwrap();
            let listener = UnixListener::bind(dir.join("cursor-clip.sock")).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let stream = stream.unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    if tx
                        .send((
                            serde_json::from_str::<FrontendMessage>(&line).unwrap(),
                            stream,
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
            });
            let next_request = || until(|| rx.try_recv().ok());
            let content = generate_overlay_content(UserConfig::default());
            let controls = &content.persistence;
            let window = adw::ApplicationWindow::builder()
                .title("Cursor Clip focus test")
                .build();
            window.set_content(Some(&content.overlay));
            window.connect_map({
                let controls = controls.clone();
                move |_| open_history(&controls)
            });
            window.present();
            let mut initial_history = None;
            let mut query = None;
            for _ in 0..2 {
                match next_request() {
                    (FrontendMessage::GetHistory, stream) => initial_history = Some(stream),
                    (FrontendMessage::GetHistoryPersistence, stream) => query = Some(stream),
                    (message, _) => panic!("Unexpected startup request {message:?}"),
                }
            }
            // GTK dispatched both futures while neither backend reply was available.
            assert!(controls.list_box.is_sensitive());
            reply(initial_history.unwrap(), history());
            until(|| controls.list_box.selected_row());
            assert!(
                controls.list_box.root().and_then(|root| root.focus())
                    == controls.list_box.selected_row().map(|row| row.upcast()),
                "First row must receive keyboard focus on cold open"
            );
            reply(
                query.unwrap(),
                BackendMessage::PersistenceState { enabled: false },
            );
            until(|| controls.check.is_sensitive().then_some(()));

            window.set_visible(false);
            window.present();
            let mut reopened = None;
            for _ in 0..2 {
                match next_request() {
                    (FrontendMessage::GetHistory, stream) => reopened = Some(stream),
                    (FrontendMessage::GetHistoryPersistence, stream) => {
                        reply(stream, BackendMessage::PersistenceState { enabled: false })
                    }
                    (message, _) => panic!("Unexpected reopen request {message:?}"),
                }
            }
            assert!(!controls.history_state.can_act_on_rows());
            let BackendMessage::History { mut items } = history() else {
                unreachable!()
            };
            items.insert(
                0,
                ClipboardItemPreview {
                    item_id: 2,
                    content_preview: "newest".into(),
                    ..items[0].clone()
                },
            );
            reply(reopened.unwrap(), BackendMessage::History { items });
            until(|| {
                (controls.history_state.can_act_on_rows() && controls.check.is_sensitive())
                    .then_some(())
            });
            assert_eq!(selected_item_id(&controls.list_box), Some(2));
            assert!(
                controls.list_box.root().and_then(|root| root.focus())
                    == controls.list_box.selected_row().map(|row| row.upcast())
            );

            controls.check.set_active(true);
            let (request, transition) = next_request();
            assert!(matches!(
                request,
                FrontendMessage::SetHistoryPersistence { enabled: true }
            ));
            assert!(persistence_pending());
            assert!(!controls.list_box.is_sensitive());
            reply(
                transition,
                BackendMessage::PersistenceState { enabled: true },
            );
            let (request, refresh) = next_request();
            assert!(matches!(request, FrontendMessage::GetHistory));
            assert!(!persistence_pending());
            assert!(controls.check.is_active() && controls.check.is_sensitive());
            assert!(controls.list_box.is_sensitive() && controls.clear_button.is_sensitive());
            focus_search_entry(&content.search_entry);
            reply(refresh, history());
            let generation = controls.history_state.render_generation.get();
            until(|| (controls.history_state.render_generation.get() != generation).then_some(()));
            assert!(
                content
                    .search_entry
                    .root()
                    .and_then(|root| root.focus())
                    .is_some_and(|focus| focus.is_ancestor(&content.search_entry))
            );

            controls.check.set_active(false);
            let (_, rejected) = next_request();
            reply(
                rejected,
                BackendMessage::Error {
                    message: "cannot save setting".into(),
                },
            );
            until(|| (!persistence_pending()).then_some(()));
            assert!(controls.check.is_active() && !controls.check.is_inconsistent());
            let (_, refresh) = next_request();
            drop(refresh);
            until(|| controls.history_status.get_visible().then_some(()));

            controls.check.set_active(false);
            let (_, unconfirmed) = next_request();
            drop(unconfirmed);
            until(|| (!persistence_pending()).then_some(()));
            assert!(controls.check.is_inconsistent());
            let (_, stalled) = next_request();
            OVERLAY_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
            request_quit();
            assert!(
                !window.is_visible(),
                "A stalled history refresh held the window open"
            );
            OVERLAY_WINDOW.with(|slot| slot.borrow_mut().take());
            drop(stalled);
            reset_close_flags();
        });
    }
}
