use gtk4::prelude::*;
use gtk4::{Application, Button, CheckButton, Label, Box, Orientation, Align, Revealer, Overlay};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::RefCell;
use std::rc::Rc;
use std::path::PathBuf;
use std::fs;
use serde::{Deserialize, Serialize};
use crate::shared::{ClipboardItemPreview, ClipboardContentType};
use crate::frontend::ipc_client::FrontendClient;
use log::{info, debug, warn, error};

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
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            show_trash: true,
            show_pin: true,
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
        && let Err(e) = fs::create_dir_all(parent) {
        warn!("Failed to create config directory: {}", e);
    }

    if let Ok(contents) = fs::read_to_string(&path)
        && let Ok(config) = toml::from_str::<UserConfig>(&contents) {
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
    let contents = toml::to_string_pretty(config).unwrap_or_else(|_| "show_trash = true\n".to_string());
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

pub fn init_clipboard_overlay(x: f64, y: f64, prefetched_items: Vec<ClipboardItemPreview>) -> Result<(), std::boxed::Box<dyn std::error::Error + Send + Sync>> {
    INIT.call_once(|| {
        adw::init().expect("Failed to initialize libadwaita");
    });

    // Create the application (was returned from init_application())
    let app: Application = adw::Application::builder()
        .application_id("com.cursor-clip")
        .build()
        .upcast();
    
    let app_clone = app.clone();
    app.connect_activate(move |_| {
        let window = create_layer_shell_window(&app_clone, x, y, prefetched_items.clone());
        
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

/// Create and configure the sync layer shell window
fn create_layer_shell_window(
    app: &Application, 
    x: f64, 
    y: f64,
    prefetched_items: Vec<ClipboardItemPreview>
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
    
    window.set_exclusive_zone(-1); 

    // Make window keyboard interactive
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);


    // Apply custom styling
    apply_custom_styling(&window);

    // Create and set content (also obtain list_box for navigation)
    let (content, list_box, items_state) = generate_overlay_content(prefetched_items);
    window.set_content(Some(&content));

    // Add key controller (Esc/j/k/Enter navigation & activation)
    let key_controller = generate_key_controller(&list_box, &items_state);
    window.add_controller(key_controller);

    // Add close request handler to ensure any window close goes through our logic
    window.connect_close_request(|_window| {
        println!("Window close requested - ensuring both overlay and capture layer close");
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
) -> (Overlay, gtk4::ListBox, Rc<RefCell<Vec<ClipboardItemPreview>>>) {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar 
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Clipboard History"))));
    // Use standard end title buttons (includes the normal close button with Adwaita styling)
    header_bar.set_show_end_title_buttons(true);
    header_bar.set_show_start_title_buttons(false);
    
    let config_state = Rc::new(RefCell::new(load_or_create_config()));
    let show_trash_default = config_state.borrow().show_trash;
    let show_pin_default = config_state.borrow().show_pin;

    // Add a three-dot menu button (icon-only) next to the close button on the right
    let three_dot_menu = Button::builder()
        .icon_name("view-more-symbolic")
        .build();
    three_dot_menu.add_css_class("flat");
    three_dot_menu.set_tooltip_text(Some("Options"));

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
    menu_revealer.set_child(Some(&menu_box));
    header_bar.pack_end(&three_dot_menu);
    
    // Add clear all button to header
    let clear_button = Button::with_label("Clear All");
    clear_button.add_css_class("destructive-action");
    header_bar.pack_start(&clear_button);

    main_box.append(&header_bar);

    // Create scrolled window for the clipboard list
    let scrolled_window = gtk4::ScrolledWindow::new();
    scrolled_window.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled_window.set_min_content_width(200);
    scrolled_window.set_min_content_height(400);

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

    let items_state = Rc::new(RefCell::new(prefetched_items));

    // Populate the list with clipboard items
    {
        let items = items_state.borrow();
        for item in items.iter() {
            let row = generate_listboxrow_from_preview(
                item,
                &list_box,
                &items_state,
                show_trash_default,
                show_pin_default,
            );
            list_box.append(&row);
        }
    }

    // If no items, show a placeholder
    if items_state.borrow().is_empty() {
        list_box.append(&make_placeholder_row());
    }

    // Handle item activation (Enter/Space/double-click) instead of mere selection
    let items_for_activation = items_state.clone();
    list_box.connect_row_activated(move |_, row| {
        let index = row.index() as usize;
        let items = items_for_activation.borrow();
        if index < items.len() {
            let item = &items[index];
            debug!("Activated clipboard item ID {}: {}", item.item_id, item.content_preview);

            match FrontendClient::new() {
                Ok(mut client) => {
                    if let Err(e) = client.set_clipboard_by_id(item.item_id) {
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
    toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        {
            let mut config = config_for_toggle.borrow_mut();
            config.show_trash = state;
            if let Err(e) = save_config(&config) {
                warn!("Failed to save config: {}", e);
            }
        }
        set_delete_buttons_visible(&list_box_for_toggle, state);
    });

    let list_box_for_pin_toggle = list_box.clone();
    let config_for_pin_toggle = config_state.clone();
    pin_toggle_check.connect_toggled(move |check| {
        let state = check.is_active();
        {
            let mut config = config_for_pin_toggle.borrow_mut();
            config.show_pin = state;
            if let Err(e) = save_config(&config) {
                warn!("Failed to save config: {}", e);
            }
        }
        set_pin_icons_visible(&list_box_for_pin_toggle, state);
    });

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

    (overlay, list_box, items_state)
}

/// Build the key controller handling Esc (close), j/k or arrows (navigate) and Enter (activate)
fn generate_key_controller(
    list_box: &gtk4::ListBox,
    items_state: &Rc<RefCell<Vec<ClipboardItemPreview>>>,
) -> gtk4::EventControllerKey {
    let controller = gtk4::EventControllerKey::new();
    let list_box_for_keys = list_box.clone();
    let items_state_for_keys = items_state.clone();
    controller.connect_key_pressed(move |_, key, _, _| {
        use gtk4::gdk::Key;
        match key {
            Key::Escape => {
                request_quit();
                gtk4::glib::Propagation::Stop
            }
            Key::j | Key::J | Key::Down => {
                if let Some(current) = list_box_for_keys.selected_row() {
                    let next_index = current.index() + 1;
                    if let Some(next_row) = list_box_for_keys.row_at_index(next_index) {
                        list_box_for_keys.select_row(Some(&next_row));
                        next_row.grab_focus();
                    }
                } else if let Some(first_row) = list_box_for_keys.row_at_index(0) {
                    list_box_for_keys.select_row(Some(&first_row));
                    first_row.grab_focus();
                }
                gtk4::glib::Propagation::Stop
            }
            Key::k | Key::K | Key::Up => {
                if let Some(current) = list_box_for_keys.selected_row() {
                    if current.index() > 0 {
                        let prev_index = current.index() - 1;
                        if let Some(prev_row) = list_box_for_keys.row_at_index(prev_index) {
                            list_box_for_keys.select_row(Some(&prev_row));
                            prev_row.grab_focus();
                        }
                    }
                } else if let Some(first_row) = list_box_for_keys.row_at_index(0) {
                    list_box_for_keys.select_row(Some(&first_row));
                    first_row.grab_focus();
                }
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
                if let Some(row) = list_box_for_keys.selected_row() {
                    let index = row.index() as usize;
                    let item_id = {
                        let items = items_state_for_keys.borrow();
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
                        let mut items = items_state_for_keys.borrow_mut();
                        if index < items.len() {
                            items.remove(index);
                        }
                    }

                    list_box_for_keys.remove(&row);

                    if items_state_for_keys.borrow().is_empty() {
                        list_box_for_keys.append(&make_placeholder_row());
                    }
                    return gtk4::glib::Propagation::Stop;
                }
                gtk4::glib::Propagation::Proceed
            }
            Key::p | Key::P => {
                if let Some(row) = list_box_for_keys.selected_row() {
                    let index = row.index() as usize;
                    let (item_id, pinned, target_index) = {
                        let mut items = items_state_for_keys.borrow_mut();
                        if index >= items.len() {
                            return gtk4::glib::Propagation::Stop;
                        }
                        let mut item = items.remove(index);
                        let new_pinned = !item.pinned;
                        item.pinned = new_pinned;
                        let insert_index = if new_pinned {
                            0
                        } else {
                            items
                                .iter()
                                .position(|existing| !existing.pinned)
                                .unwrap_or(items.len())
                        };
                        let item_id = item.item_id;
                        items.insert(insert_index, item);
                        (item_id, new_pinned, insert_index)
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

                    list_box_for_keys.remove(&row);
                    list_box_for_keys.insert(&row, target_index as i32);
                    list_box_for_keys.select_row(Some(&row));
                    row.grab_focus();

                    if let Some(pin_button) = find_button_in_row(&row, "clipboard-pin") {
                        if pinned {
                            pin_button.add_css_class("pinned");
                            pin_button.set_tooltip_text(Some("Unpin"));
                        } else {
                            pin_button.remove_css_class("pinned");
                            pin_button.set_tooltip_text(Some("Pin"));
                        }
                    }
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
    css_provider.load_from_data(
        "
        window {
            border-radius: 12px;
            background: #222226;
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
            border: 2px solid transparent;
            border-radius: 10px;
            padding: 4px 4px;
            margin: 6px 12px;
            transition: border-color 150ms ease, box-shadow 150ms ease, background 150ms ease;
        }

        .clipboard-item:hover {
            border-color: #3584E4;
            background: shade(#343437, 1.05);
        }

        .clipboard-item:selected {
            border-color: #3584E4;
            background: alpha(#3584E4, 0.18);
        }

        .clipboard-preview {
            opacity: 0.9;
        }

        .clipboard-preview.monospace {
            font-family: monospace;
        }

        .clipboard-time {
            font-size: 0.8em;
            opacity: 0.6;
        }

        .clipboard-delete {
            color: #bfc3c7;
            padding: 2px 4px;
        }

        .clipboard-pin {
            color: #bfc3c7;
            padding: 2px 4px;
        }

        .clipboard-item:hover .clipboard-delete,
        .clipboard-delete:hover {
            color: #ffffff;
        }

        .clipboard-item:hover .clipboard-pin {
            color: #ffffff;
        }

        .clipboard-pin:hover {
            color: #ffffff;
        }

        .clipboard-pin.pinned {
            color: #ffffff;
        }

        .menu-revealer {
            background: #2b2b2f;
            border-radius: 8px;
            padding: 6px 8px;
        }
        "
    );

    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Create a clipboard history item row from backend data
fn generate_listboxrow_from_preview(
    item: &ClipboardItemPreview,
    list_box: &gtk4::ListBox,
    items_state: &Rc<RefCell<Vec<ClipboardItemPreview>>>,
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

    let pin_button = Button::builder()
        .icon_name("view-pin-symbolic")
        .build();
    pin_button.add_css_class("flat");
    pin_button.add_css_class("clipboard-pin");
    if item.pinned {
        pin_button.add_css_class("pinned");
        pin_button.set_tooltip_text(Some("Unpin"));
    } else {
        pin_button.set_tooltip_text(Some("Pin"));
    }
    pin_button.set_visible(show_pin);

    let delete_button = Button::builder()
        .icon_name("user-trash-symbolic")
        .build();
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
    let items_state = items_state.clone();
    let row_weak = row.downgrade();
    let item_id = item.item_id;
    let list_box_for_delete = list_box.clone();
    let items_state_for_delete = items_state.clone();
    let row_weak_for_delete = row_weak.clone();
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
            let mut items = items_state_for_delete.borrow_mut();
            if let Some(index) = items.iter().position(|entry| entry.item_id == item_id) {
                items.remove(index);
            }
        }

        if let Some(row) = row_weak_for_delete.upgrade() {
            list_box_for_delete.remove(&row);
        }

        if items_state_for_delete.borrow().is_empty() {
            list_box_for_delete.append(&make_placeholder_row());
        }
    });
    let list_box_for_pin = list_box.clone();
    let items_state_for_pin = items_state.clone();
    let row_weak_for_pin = row_weak.clone();
    pin_button.connect_clicked(move |_| {
        let row = match row_weak_for_pin.upgrade() {
            Some(row) => row,
            None => return,
        };
        let index = row.index() as usize;
        let (item_id, pinned, target_index) = {
            let mut items = items_state_for_pin.borrow_mut();
            if index >= items.len() {
                return;
            }
            let mut item = items.remove(index);
            let new_pinned = !item.pinned;
            item.pinned = new_pinned;
            let insert_index = if new_pinned {
                0
            } else {
                items
                    .iter()
                    .position(|existing| !existing.pinned)
                    .unwrap_or(items.len())
            };
            let item_id = item.item_id;
            items.insert(insert_index, item);
            (item_id, new_pinned, insert_index)
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

        list_box_for_pin.remove(&row);
        list_box_for_pin.insert(&row, target_index as i32);
        list_box_for_pin.select_row(Some(&row));
        row.grab_focus();
        if let Some(pin_button) = find_button_in_row(&row, "clipboard-pin") {
            if pinned {
                pin_button.add_css_class("pinned");
                pin_button.set_tooltip_text(Some("Unpin"));
            } else {
                pin_button.remove_css_class("pinned");
                pin_button.set_tooltip_text(Some("Pin"));
            }
        }
        debug!("Updated pinned state for clipboard item ID {}", item_id);
    });
    row
}

fn make_placeholder_row() -> gtk4::ListBoxRow {
    let placeholder_row = gtk4::ListBoxRow::new();
    let placeholder_label = Label::new(Some("No clipboard history yet"));
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
            && let Some(delete_button) = find_button_in_row(&row, "clipboard-delete") {
            delete_button.set_visible(visible);
        }
        child = widget.next_sibling();
    }
}

fn set_pin_icons_visible(list_box: &gtk4::ListBox, visible: bool) {
    let mut child = list_box.first_child();
    while let Some(widget) = child {
        if let Ok(row) = widget.clone().downcast::<gtk4::ListBoxRow>()
            && let Some(pin_button) = find_button_in_row(&row, "clipboard-pin") {
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
        format!("{} minute{} ago", minutes, if minutes == 1 { "" } else { "s" })
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else {
        let days = diff / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }
}
