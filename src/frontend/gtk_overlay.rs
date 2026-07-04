use crate::frontend::ipc_client::FrontendClient;
use crate::shared::{ClipboardContentType, ClipboardItemPreview};
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box, Button, CheckButton, Label, Orientation, Overlay, Revealer,
    SearchEntry,
};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

static INIT: Once = Once::new();
pub static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

// Thread-local storage for the overlay state since GTK objects aren't Send/Sync
thread_local! {
    static OVERLAY_WINDOW: RefCell<Option<adw::ApplicationWindow>> = const { RefCell::new(None) };
    static OVERLAY_APP: RefCell<Option<Application>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct UserConfig {
    show_trash: bool,
    show_pin: bool,
    #[serde(alias = "persistent_history")]
    persistence_enabled: bool,
    instant_paste: bool,
}

#[derive(Clone)]
struct HistoryListState {
    items: Rc<RefCell<Vec<ClipboardItemPreview>>>,
    all_items: Rc<RefCell<Vec<ClipboardItemPreview>>>,
    search_query: Rc<RefCell<String>>,
    show_trash: Rc<RefCell<bool>>,
    show_pin: Rc<RefCell<bool>>,
}

struct OverlayContent {
    overlay: Overlay,
    list_box: gtk4::ListBox,
    history_state: HistoryListState,
    search_entry: SearchEntry,
    search_revealer: Revealer,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            show_trash: true,
            show_pin: true,
            persistence_enabled: false,
            instant_paste: true,
        }
    }
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("cursor-clip")
        .join("config.toml")
}

fn load_or_create_config() -> UserConfig {
    let path = config_path();
    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        warn!("Failed to create config directory: {}", e);
    }

    if let Ok(contents) = fs::read_to_string(&path)
        && let Ok(config) = toml::from_str::<UserConfig>(&contents)
    {
        return config;
    }

    let config = UserConfig::default();
    if let Err(e) = save_config(&config) {
        warn!("Failed to write default config: {}", e);
    }
    config
}

fn save_config(config: &UserConfig) -> Result<(), std::io::Error> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, contents)
}

pub fn is_close_requested() -> bool {
    CLOSE_REQUESTED.load(Ordering::Relaxed)
}

pub fn reset_close_flags() {
    CLOSE_REQUESTED.store(false, Ordering::Relaxed);
}

// Centralized quit path to avoid double-close reentrancy and ensure flags + app quit
fn request_quit() {
    CLOSE_REQUESTED.store(true, Ordering::Relaxed);
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

pub fn init_clipboard_overlay(
    x: f64,
    y: f64,
    overlay_width: i32,
    overlay_height: i32,
    monitor_width: i32,
    monitor_height: i32,
    prefetched_items: Vec<ClipboardItemPreview>,
) -> Result<(), std::boxed::Box<dyn std::error::Error + Send + Sync>> {
    INIT.call_once(|| {
        adw::init().expect("Failed to initialize libadwaita");
    });
    configure_color_scheme();

    // Create the application (was returned from init_application())
    let app: Application = adw::Application::builder()
        .application_id("com.cursor-clip")
        .build()
        .upcast();

    let app_clone = app.clone();
    app.connect_activate(move |_| {
        let window = create_layer_shell_window(
            &app_clone,
            x,
            y,
            overlay_width,
            overlay_height,
            monitor_width,
            monitor_height,
            prefetched_items.clone(),
        );

        // Store the window in our thread-local storage
        OVERLAY_WINDOW.with(|w| {
            *w.borrow_mut() = Some(window.clone());
        });

        OVERLAY_APP.with(|a| {
            *a.borrow_mut() = Some(app_clone.clone());
        });

        window.present();

        debug!("Libadwaita overlay window created at ({}, {})", x, y);
    });

    // Run the application
    app.run_with_args::<String>(&[]);

    // Belt-and-suspenders: clear TLS after run returns
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
    x: f64,
    y: f64,
    overlay_width: i32,
    overlay_height: i32,
    monitor_width: i32,
    monitor_height: i32,
    prefetched_items: Vec<ClipboardItemPreview>,
) -> adw::ApplicationWindow {
    // Create the main window using Adwaita ApplicationWindow
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Clipboard History")
        .decorated(false)
        .build();

    // Initialize layer shell for this window
    window.init_layer_shell();

    // Configure layer shell properties
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("cursor-clip"));

    // Anchor to top-left corner for precise positioning
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);

    // Set margins to position the window at the specified coordinates
    window.set_margin(Edge::Top, y as i32);
    window.set_margin(Edge::Left, x as i32);

    // clamp with the real allocated size to avoid off-screen spawn.
    if monitor_width > 0 && monitor_height > 0 {
        window.connect_map(move |mapped_window| {
            let mapped_window = mapped_window.clone();
            gtk4::glib::idle_add_local_once(move || {
                let margin = 5.0;
                let window_width = mapped_window.allocated_width().max(overlay_width) as f64;
                let window_height = mapped_window.allocated_height().max(overlay_height) as f64;

                let max_x = (monitor_width as f64 - window_width - margin).max(margin);
                let max_y = (monitor_height as f64 - window_height - margin).max(margin);
                let clamped_x = x.clamp(margin, max_x) as i32;
                let clamped_y = y.clamp(margin, max_y) as i32;

                mapped_window.set_margin(Edge::Top, clamped_y);
                mapped_window.set_margin(Edge::Left, clamped_x);
            });
        });
    }

    window.set_exclusive_zone(-1);

    // Make window keyboard interactive
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

    // Apply custom styling
    apply_custom_styling(&window);

    // Create and set content (also obtain list_box for navigation)
    let content = generate_overlay_content(prefetched_items, overlay_width, overlay_height);
    window.set_content(Some(&content.overlay));

    // Add key controller (Esc/j/k/Enter navigation & activation)
    let key_controller = generate_key_controller(
        &content.list_box,
        &content.history_state,
        &content.search_entry,
        &content.search_revealer,
    );
    window.add_controller(key_controller);

    // Add close request handler to ensure any window close goes through our logic
    window.connect_close_request(|_window| {
        debug!("Window close requested - closing overlay and capture layer");
        request_quit();
        // Stop default handler to avoid double-close reentrancy during teardown
        gtk4::glib::Propagation::Stop
    });

    window
}

/// Create a Windows 11-style clipboard history list with provided (prefetched) backend data.
/// Falls back to a lazy on-demand fetch only if the provided vector is empty.
fn generate_overlay_content(
    mut prefetched_items: Vec<ClipboardItemPreview>,
    overlay_width: i32,
    overlay_height: i32,
) -> OverlayContent {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Clipboard History"))));
    // Layer-shell + undecorated windows can render built-in title buttons unreliably.
    // Use an explicit close button styled like a normal Adwaita title button instead.
    header_bar.set_show_end_title_buttons(false);
    header_bar.set_show_start_title_buttons(false);

    let config_state = Rc::new(RefCell::new(load_or_create_config()));
    let show_trash_default = config_state.borrow().show_trash;
    let show_pin_default = config_state.borrow().show_pin;
    let persistence_enabled_default = config_state.borrow().persistence_enabled;
    let instant_paste_default = config_state.borrow().instant_paste;
    let show_trash_state = Rc::new(RefCell::new(show_trash_default));
    let show_pin_state = Rc::new(RefCell::new(show_pin_default));

    if let Ok(mut client) = FrontendClient::new()
        && let Err(e) = client.set_persistence_enabled(persistence_enabled_default)
    {
        warn!("Failed to sync persistence setting with backend: {}", e);
    }

    // Add right-side header actions (search + menu + close)
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
    header_action_group.append(&search_button);
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
    menu_revealer.set_margin_end(10);
    menu_revealer.add_css_class("menu-revealer");

    let menu_box = Box::new(Orientation::Vertical, 8);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(10);
    menu_box.set_margin_end(10);

    let toggle_row = Box::new(Orientation::Horizontal, 8);
    let toggle_label = Label::new(Some("Show delete button"));
    toggle_label.set_halign(Align::Start);
    toggle_label.set_hexpand(true);
    let toggle_check = CheckButton::new();
    toggle_check.set_active(show_trash_default);
    toggle_row.append(&toggle_label);
    toggle_row.append(&toggle_check);
    menu_box.append(&toggle_row);

    let pin_toggle_row = Box::new(Orientation::Horizontal, 8);
    let pin_toggle_label = Label::new(Some("Show pin icon"));
    pin_toggle_label.set_halign(Align::Start);
    pin_toggle_label.set_hexpand(true);
    let pin_toggle_check = CheckButton::new();
    pin_toggle_check.set_active(show_pin_default);
    pin_toggle_row.append(&pin_toggle_label);
    pin_toggle_row.append(&pin_toggle_check);
    menu_box.append(&pin_toggle_row);

    let persistence_toggle_row = Box::new(Orientation::Horizontal, 8);
    let persistence_toggle_label = Label::new(Some("Persistent history"));
    persistence_toggle_label.set_halign(Align::Start);
    persistence_toggle_label.set_hexpand(true);
    let persistence_toggle_check = CheckButton::new();
    persistence_toggle_check.set_active(persistence_enabled_default);
    persistence_toggle_row.append(&persistence_toggle_label);
    persistence_toggle_row.append(&persistence_toggle_check);
    menu_box.append(&persistence_toggle_row);

    let instant_paste_toggle_row = Box::new(Orientation::Horizontal, 8);
    let instant_paste_toggle_label = Label::new(Some("Instant paste"));
    instant_paste_toggle_label.set_halign(Align::Start);
    instant_paste_toggle_label.set_hexpand(true);
    let instant_paste_toggle_check = CheckButton::new();
    instant_paste_toggle_check.set_active(instant_paste_default);
    instant_paste_toggle_row.append(&instant_paste_toggle_label);
    instant_paste_toggle_row.append(&instant_paste_toggle_check);
    menu_box.append(&instant_paste_toggle_row);

    menu_revealer.set_child(Some(&menu_box));
    header_bar.pack_end(&close_button);
    header_bar.pack_end(&header_action_group);

    close_button.connect_clicked(move |_| {
        request_quit();
    });

    // Add clear all button to header
    let clear_button = Button::with_label("Clear All");
    clear_button.add_css_class("destructive-action");
    header_bar.pack_start(&clear_button);

    main_box.append(&header_bar);

    let search_revealer = Revealer::new();
    search_revealer.set_reveal_child(false);
    search_revealer.set_visible(false);
    search_revealer.set_transition_duration(120);
    search_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search clipboard history"));
    search_entry.set_margin_start(12);
    search_entry.set_margin_end(12);
    search_entry.set_margin_bottom(6);
    search_entry.add_css_class("clipboard-search");
    search_revealer.set_child(Some(&search_entry));
    main_box.append(&search_revealer);

    let search_revealer_for_button = search_revealer.clone();
    let search_entry_for_button = search_entry.clone();
    search_button.connect_clicked(move |_| {
        let next_state = !search_revealer_for_button.is_child_revealed();
        search_revealer_for_button.set_visible(next_state);
        search_revealer_for_button.set_reveal_child(next_state);

        if next_state {
            search_entry_for_button.grab_focus();
            search_entry_for_button.select_region(0, -1);
        }
    });

    // Create scrolled window for the clipboard list
    let scrolled_window = gtk4::ScrolledWindow::new();
    scrolled_window.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled_window.set_min_content_width(overlay_width);
    scrolled_window.set_min_content_height(overlay_height);

    // Create list box for clipboard items
    let list_box = gtk4::ListBox::new();
    // Use custom styling instead of the default boxed-list to create floating cards
    list_box.add_css_class("clipboard-list");
    //list_box.set_margin_top(6);
    list_box.set_margin_bottom(6);
    list_box.set_margin_start(4);
    list_box.set_margin_end(4);
    list_box.set_selection_mode(gtk4::SelectionMode::Single);

    // Start with prefetched items; if empty try one lazy fetch (non-fatal if it fails)

    if prefetched_items.is_empty() {
        debug!("Prefetched clipboard history empty - trying on-demand fetch...");
        if let Ok(mut client) = FrontendClient::new() {
            match client.get_history() {
                Ok(fetched) => prefetched_items = fetched,
                Err(e) => warn!("Error fetching clipboard history on-demand: {}", e),
            }
        }
    }

    let history_state = HistoryListState {
        items: Rc::new(RefCell::new(Vec::new())),
        all_items: Rc::new(RefCell::new(prefetched_items)),
        search_query: Rc::new(RefCell::new(String::new())),
        show_trash: show_trash_state,
        show_pin: show_pin_state,
    };

    rebuild_list(&list_box, &history_state);
    select_first_row(&list_box);

    // Handle item activation (Enter/Space/double-click) instead of mere selection
    let history_state_for_activation = history_state.clone();
    let config_for_activation = config_state.clone();
    list_box.connect_row_activated(move |_, row| {
        let index = row.index() as usize;
        let items = history_state_for_activation.items.borrow();
        if index < items.len() {
            let item = &items[index];
            let instant_paste = config_for_activation.borrow().instant_paste;
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
    let config_for_toggle = config_state.clone();
    let history_state_for_toggle = history_state.clone();
    toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        {
            let mut config = config_for_toggle.borrow_mut();
            config.show_trash = state;
            if let Err(e) = save_config(&config) {
                warn!("Failed to save config: {}", e);
            }
        }
        *history_state_for_toggle.show_trash.borrow_mut() = state;
        set_delete_buttons_visible(&list_box_for_toggle, state);
    });

    let list_box_for_pin_toggle = list_box.clone();
    let config_for_pin_toggle = config_state.clone();
    let history_state_for_pin_toggle = history_state.clone();
    pin_toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        {
            let mut config = config_for_pin_toggle.borrow_mut();
            config.show_pin = state;
            if let Err(e) = save_config(&config) {
                warn!("Failed to save config: {}", e);
            }
        }
        *history_state_for_pin_toggle.show_pin.borrow_mut() = state;
        set_pin_icons_visible(&list_box_for_pin_toggle, state);
    });

    let config_for_persistence_toggle = config_state.clone();
    persistence_toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        {
            let mut config = config_for_persistence_toggle.borrow_mut();
            config.persistence_enabled = state;
            if let Err(e) = save_config(&config) {
                warn!("Failed to save config: {}", e);
            }
        }

        match FrontendClient::new() {
            Ok(mut client) => {
                if let Err(e) = client.set_persistence_enabled(state) {
                    warn!("Failed to update persistence in backend: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to connect to backend for persistence toggle: {}", e);
            }
        }
    });

    let config_for_instant_paste_toggle = config_state.clone();
    instant_paste_toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        let mut config = config_for_instant_paste_toggle.borrow_mut();
        config.instant_paste = state;
        if let Err(e) = save_config(&config) {
            warn!("Failed to save config: {}", e);
        }
    });

    let list_box_for_search = list_box.clone();
    let history_state_for_search = history_state.clone();
    search_entry.connect_search_changed(move |entry| {
        *history_state_for_search.search_query.borrow_mut() = entry.text().to_string();
        rebuild_list(&list_box_for_search, &history_state_for_search);
        select_first_row_without_focus(&list_box_for_search);
    });

    let list_box_for_search_activate = list_box.clone();
    search_entry.connect_activate(move |_| {
        if let Some(row) = list_box_for_search_activate.selected_row() {
            row.emit_by_name::<()>("activate", &[]);
        }
    });

    let list_box_for_stop_search = list_box.clone();
    search_entry.connect_stop_search(move |_| {
        if list_box_for_stop_search.selected_row().is_none() {
            select_first_row(&list_box_for_stop_search);
        } else {
            list_box_for_stop_search.grab_focus();
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

    // Connect button signals
    clear_button.connect_clicked(move |_| {
        match FrontendClient::new() {
            Ok(mut client) => {
                if let Err(e) = client.clear_history() {
                    error!("Error clearing clipboard history: {}", e);
                } else {
                    info!("Clipboard history cleared");
                    // Close the overlay after clearing
                    request_quit();
                }
            }
            Err(e) => {
                error!("Error creating frontend client: {}", e);
            }
        }
    });

    let overlay = Overlay::new();
    overlay.set_child(Some(&main_box));
    overlay.add_overlay(&menu_revealer);

    OverlayContent {
        overlay,
        list_box,
        history_state,
        search_entry,
        search_revealer,
    }
}

/// Build the key controller handling Esc (close), j/k or arrows (navigate) and Enter (activate)
fn generate_key_controller(
    list_box: &gtk4::ListBox,
    history_state: &HistoryListState,
    search_entry: &SearchEntry,
    search_revealer: &Revealer,
) -> gtk4::EventControllerKey {
    let controller = gtk4::EventControllerKey::new();
    let list_box_for_keys = list_box.clone();
    let history_state_for_keys = history_state.clone();
    let search_entry_for_keys = search_entry.clone();
    let search_revealer_for_keys = search_revealer.clone();
    controller.connect_key_pressed(move |_, key, _, _| {
        use gtk4::gdk::Key;
        match key {
            Key::Escape => {
                if search_revealer_for_keys.is_child_revealed() && search_entry_for_keys.has_focus()
                {
                    if list_box_for_keys.selected_row().is_none() {
                        select_first_row(&list_box_for_keys);
                    } else {
                        list_box_for_keys.grab_focus();
                    }
                    return gtk4::glib::Propagation::Stop;
                }
                request_quit();
                gtk4::glib::Propagation::Stop
            }
            Key::slash => {
                if search_entry_for_keys.has_focus() {
                    return gtk4::glib::Propagation::Proceed;
                }
                search_revealer_for_keys.set_visible(true);
                search_revealer_for_keys.set_reveal_child(true);
                search_entry_for_keys.grab_focus();
                gtk4::glib::Propagation::Stop
            }
            Key::j | Key::J | Key::Down => {
                if matches!(key, Key::j | Key::J) && search_entry_for_keys.has_focus() {
                    return gtk4::glib::Propagation::Proceed;
                }
                if key == Key::Down && search_entry_for_keys.has_focus() {
                    list_box_for_keys.grab_focus();
                }
                select_next_row(&list_box_for_keys, false);
                gtk4::glib::Propagation::Stop
            }
            Key::k | Key::K | Key::Up => {
                if matches!(key, Key::k | Key::K) && search_entry_for_keys.has_focus() {
                    return gtk4::glib::Propagation::Proceed;
                }
                if key == Key::Up && search_entry_for_keys.has_focus() {
                    list_box_for_keys.grab_focus();
                }
                select_previous_row(&list_box_for_keys, false);
                gtk4::glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter => {
                if let Some(row) = list_box_for_keys.selected_row() {
                    row.emit_by_name::<()>("activate", &[]);
                    return gtk4::glib::Propagation::Stop;
                }
                gtk4::glib::Propagation::Proceed
            }
            Key::Delete => {
                if search_entry_for_keys.has_focus() {
                    return gtk4::glib::Propagation::Proceed;
                }
                if let Some(row) = list_box_for_keys.selected_row() {
                    let index = row.index() as usize;
                    let item_id = {
                        let items = history_state_for_keys.items.borrow();
                        if index >= items.len() {
                            return gtk4::glib::Propagation::Stop;
                        }
                        items[index].item_id
                    };

                    match FrontendClient::new() {
                        Ok(mut client) => {
                            if let Err(e) = client.delete_item_by_id(item_id) {
                                error!("Error deleting clipboard item by ID: {}", e);
                                return gtk4::glib::Propagation::Stop;
                            }
                        }
                        Err(e) => {
                            error!("Error creating frontend client: {}", e);
                            return gtk4::glib::Propagation::Stop;
                        }
                    }

                    {
                        let mut items = history_state_for_keys.all_items.borrow_mut();
                        if let Some(index) = items.iter().position(|item| item.item_id == item_id) {
                            items.remove(index);
                        }
                    }

                    rebuild_list(&list_box_for_keys, &history_state_for_keys);
                    select_first_row(&list_box_for_keys);
                    return gtk4::glib::Propagation::Stop;
                }
                gtk4::glib::Propagation::Proceed
            }
            Key::p | Key::P => {
                if search_entry_for_keys.has_focus() {
                    return gtk4::glib::Propagation::Proceed;
                }
                if let Some(row) = list_box_for_keys.selected_row() {
                    let index = row.index() as usize;
                    let item_id = {
                        let items = history_state_for_keys.items.borrow();
                        if index >= items.len() {
                            return gtk4::glib::Propagation::Stop;
                        }
                        items[index].item_id
                    };

                    let Some(pinned) = next_pinned_state(&history_state_for_keys, item_id) else {
                        return gtk4::glib::Propagation::Stop;
                    };

                    match FrontendClient::new() {
                        Ok(mut client) => {
                            if let Err(e) = client.set_pinned(item_id, pinned) {
                                error!("Error updating pinned state: {}", e);
                                return gtk4::glib::Propagation::Stop;
                            }
                        }
                        Err(e) => {
                            error!("Error creating frontend client: {}", e);
                            return gtk4::glib::Propagation::Stop;
                        }
                    }

                    apply_pinned_state(&history_state_for_keys, item_id, pinned);
                    rebuild_list(&list_box_for_keys, &history_state_for_keys);
                    select_row_by_item_id(&list_box_for_keys, &history_state_for_keys, item_id);
                    debug!("Updated pinned state for clipboard item ID {}", item_id);
                    return gtk4::glib::Propagation::Stop;
                }
                gtk4::glib::Propagation::Proceed
            }
            _ => gtk4::glib::Propagation::Proceed,
        }
    });
    controller
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
        window {
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
        window {
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
        let gbytes = glib::Bytes::from(bytes);
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
        let content_label = Label::new(Some(&item.content_preview));
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

    let list_box = list_box.clone();
    let history_state = history_state.clone();
    let item_id = item.item_id;
    let list_box_for_delete = list_box.clone();
    let history_state_for_delete = history_state.clone();
    delete_button.connect_clicked(move |_| {
        match FrontendClient::new() {
            Ok(mut client) => {
                if let Err(e) = client.delete_item_by_id(item_id) {
                    error!("Error deleting clipboard item by ID: {}", e);
                    return;
                }
            }
            Err(e) => {
                error!("Error creating frontend client: {}", e);
                return;
            }
        }

        {
            let mut items = history_state_for_delete.all_items.borrow_mut();
            if let Some(index) = items.iter().position(|entry| entry.item_id == item_id) {
                items.remove(index);
            }
        }

        rebuild_list(&list_box_for_delete, &history_state_for_delete);
        select_first_row(&list_box_for_delete);
    });
    let list_box_for_pin = list_box.clone();
    let history_state_for_pin = history_state.clone();
    pin_button.connect_clicked(move |_| {
        let Some(pinned) = next_pinned_state(&history_state_for_pin, item_id) else {
            return;
        };

        match FrontendClient::new() {
            Ok(mut client) => {
                if let Err(e) = client.set_pinned(item_id, pinned) {
                    error!("Error updating pinned state: {}", e);
                    return;
                }
            }
            Err(e) => {
                error!("Error creating frontend client: {}", e);
                return;
            }
        }

        apply_pinned_state(&history_state_for_pin, item_id, pinned);
        rebuild_list(&list_box_for_pin, &history_state_for_pin);
        select_row_by_item_id(&list_box_for_pin, &history_state_for_pin, item_id);
        debug!("Updated pinned state for clipboard item ID {}", item_id);
    });
    row
}

fn rebuild_list(list_box: &gtk4::ListBox, history_state: &HistoryListState) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let query = history_state.search_query.borrow().trim().to_lowercase();
    let filtered_items: Vec<ClipboardItemPreview> = history_state
        .all_items
        .borrow()
        .iter()
        .filter(|item| item_matches_query(item, &query))
        .cloned()
        .collect();

    {
        let mut visible_items = history_state.items.borrow_mut();
        *visible_items = filtered_items;
    }

    let show_trash = *history_state.show_trash.borrow();
    let show_pin = *history_state.show_pin.borrow();
    for item in history_state.items.borrow().iter() {
        let row =
            generate_listboxrow_from_preview(item, list_box, history_state, show_trash, show_pin);
        list_box.append(&row);
    }

    if history_state.items.borrow().is_empty() {
        list_box.append(&make_placeholder_row_with_message(if query.is_empty() {
            "No clipboard history yet"
        } else {
            "No matches found"
        }));
    }
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
    if let Some(row) = list_box.row_at_index(0)
        && row.is_selectable()
    {
        list_box.select_row(Some(&row));
        if grab_focus {
            row.grab_focus();
        }
        return true;
    }
    false
}

fn select_next_row(list_box: &gtk4::ListBox, wrap_to_first: bool) -> bool {
    if let Some(current) = list_box.selected_row() {
        let next_index = current.index() + 1;
        if let Some(next_row) = list_box.row_at_index(next_index) {
            list_box.select_row(Some(&next_row));
            next_row.grab_focus();
            return true;
        }

        return wrap_to_first && select_first_row_with_focus(list_box, true);
    }

    select_first_row_with_focus(list_box, true)
}

fn select_previous_row(list_box: &gtk4::ListBox, wrap_to_first: bool) -> bool {
    if let Some(current) = list_box.selected_row() {
        if current.index() > 0 {
            let prev_index = current.index() - 1;
            if let Some(prev_row) = list_box.row_at_index(prev_index) {
                list_box.select_row(Some(&prev_row));
                prev_row.grab_focus();
                return true;
            }
        }

        return wrap_to_first && select_first_row_with_focus(list_box, true);
    }

    select_first_row_with_focus(list_box, true)
}

fn select_row_by_item_id(list_box: &gtk4::ListBox, history_state: &HistoryListState, item_id: u64) {
    let Some(index) = history_state
        .items
        .borrow()
        .iter()
        .position(|item| item.item_id == item_id)
    else {
        select_first_row(list_box);
        return;
    };

    if let Some(row) = list_box.row_at_index(index as i32) {
        list_box.select_row(Some(&row));
        row.grab_focus();
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

fn make_placeholder_row_with_message(message: &str) -> gtk4::ListBoxRow {
    let placeholder_row = gtk4::ListBoxRow::new();
    let placeholder_label = Label::new(Some(message));
    placeholder_label.add_css_class("dim-label");
    placeholder_label.set_margin_top(20);
    placeholder_label.set_margin_bottom(20);
    placeholder_row.set_child(Some(&placeholder_label));
    placeholder_row.set_selectable(false);
    placeholder_row.set_activatable(false);
    placeholder_row
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
