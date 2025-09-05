use gtk4::prelude::*;
use gtk4::{Application, Button, Label, Box, Orientation, Align};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::shared::{ClipboardItemPreview, ClipboardContentType};
use crate::frontend::frontend_client::FrontendClient;

static INIT: Once = Once::new();
pub static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

// Thread-local storage for the overlay state since GTK objects aren't Send/Sync
thread_local! {
    static OVERLAY_WINDOW: RefCell<Option<adw::ApplicationWindow>> = RefCell::new(None);
    static OVERLAY_APP: RefCell<Option<Application>> = RefCell::new(None);
}

pub fn is_close_requested() -> bool {
    CLOSE_REQUESTED.load(Ordering::Relaxed)
}

pub fn reset_close_flags() {
    CLOSE_REQUESTED.store(false, Ordering::Relaxed);
}

/// Initialize the GTK/Libadwaita application
fn init_application() -> Application {
    INIT.call_once(|| {
        // Initialize libadwaita which also initializes GTK
        adw::init().expect("Failed to initialize libadwaita");
    });

    // Create the application
    let app = adw::Application::builder()
        .application_id("com.cursor-clip")
        .build();

    app.upcast()
}

/// Create a Windows 11-style clipboard history list with backend data
fn create_overlay_content() -> Box {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar 
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Clipboard History"))));
    // Use standard end title buttons (includes the normal close button with Adwaita styling)
    header_bar.set_show_end_title_buttons(true);
    header_bar.set_show_start_title_buttons(false);
    
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

    // Load clipboard items from backend
    let items = match FrontendClient::new() {
        Ok(mut client) => {
            client.get_history().unwrap_or_else(|e| {
                eprintln!("Error getting clipboard history: {}", e);
                // Fall back to sample data
                vec![
                    ClipboardItemPreview {
                        item_id: 1,
                        content_preview: "Internal Error querying the History!".to_string(),
                        content_preview_type: ClipboardContentType::Text,
                        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                    }
                ]
            })
        }
        Err(e) => {
            eprintln!("Error connecting to backend: {}", e);
            // Fall back to sample data
            vec![
                ClipboardItemPreview {
                    item_id: 1,
                    content_preview: "Backend not available - first start cursor-clip --daemon".to_string(),
                    content_preview_type: ClipboardContentType::Text,
                    timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                },
            ]
        }
    };

        // Populate the list with clipboard items
    for item in &items {
        let row = create_clipboard_item_from_backend(&item);
        list_box.append(&row);
    }

        // If no items, show a placeholder
    if items.is_empty() {
        let placeholder_row = gtk4::ListBoxRow::new();
        let placeholder_label = Label::new(Some("No clipboard history yet"));
        placeholder_label.add_css_class("dim-label");
        placeholder_label.set_margin_top(20);
        placeholder_label.set_margin_bottom(20);
        placeholder_row.set_child(Some(&placeholder_label));
        list_box.append(&placeholder_row);
    }

    // Handle item selection
    let items_for_selection = items.clone();
    list_box.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let index = row.index() as usize;
            if index < items_for_selection.len() {
                let item = &items_for_selection[index];
                println!("Selected clipboard item ID {}: {}", item.item_id, item.content_preview);
                
                // Use ID-based clipboard operation
                match FrontendClient::new() {
                    Ok(mut client) => {
                        if let Err(e) = client.set_clipboard_by_id(item.item_id) {
                            eprintln!("Error setting clipboard by ID: {}", e);
                        } else {
                            println!("Successfully set clipboard content by ID: {}", item.item_id);
                            // Close the overlay after successful selection
                            CLOSE_REQUESTED.store(true, Ordering::Relaxed);
                            OVERLAY_WINDOW.with(|window| {
                                if let Some(ref win) = *window.borrow() {
                                    win.close();
                                }
                            });
                        }
                    }
                    Err(e) => {
                        eprintln!("Error creating frontend client: {}", e);
                    }
                }
            }
        }
    });

    scrolled_window.set_child(Some(&list_box));
    main_box.append(&scrolled_window);

    // Connect button signals
    clear_button.connect_clicked(move |_| {
        println!("Clear all clipboard history");
    match FrontendClient::new() {
            Ok(mut client) => {
                if let Err(e) = client.clear_history() {
                    eprintln!("Error clearing clipboard history: {}", e);
                } else {
                    println!("Successfully cleared clipboard history");
                    // Close the overlay after clearing
                    CLOSE_REQUESTED.store(true, Ordering::Relaxed);
                    OVERLAY_WINDOW.with(|window| {
                        if let Some(ref win) = *window.borrow() {
                            win.close();
                        }
                    });
                }
            }
            Err(e) => {
                eprintln!("Error creating frontend client: {}", e);
            }
        }
    });

    main_box
}

/// Sync version of the main entry point for creating the overlay
pub fn create_clipboard_overlay(x: f64, y: f64) -> Result<(), std::boxed::Box<dyn std::error::Error + Send + Sync>> {
    let app = init_application();
    
    let app_clone = app.clone();
    app.connect_activate(move |_| {
        let window = create_layer_shell_window(&app_clone, x, y);
        
        // Store the window in our thread-local storage
        OVERLAY_WINDOW.with(|w| {
            *w.borrow_mut() = Some(window.clone());
        });
        
        OVERLAY_APP.with(|a| {
            *a.borrow_mut() = Some(app_clone.clone());
        });
        
        // Show the window
        window.present();
        
        println!("Libadwaita overlay window created and shown at ({}, {})", x, y);
    });

    // Run the application
    app.run_with_args::<String>(&[]);
    Ok(())
}

/// Create and configure the sync layer shell window
fn create_layer_shell_window(
    app: &Application, 
    x: f64, 
    y: f64
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

    // Create and set content
    let content = create_overlay_content();
    window.set_content(Some(&content));

    // Add focus-out handler to close when clicking outside
    window.connect_is_active_notify(|window| {
        println!("inside GTK window focus handler (checking outside)");
        if !window.is_active() {
            println!("GTK window lost focus - closing both overlay and capture layer");
            CLOSE_REQUESTED.store(true, Ordering::Relaxed);
            window.close();
        }
    });

    // Add close request handler to ensure any window close goes through our logic
    window.connect_close_request(|_window| {
        println!("Window close requested - ensuring both overlay and capture layer close");
        CLOSE_REQUESTED.store(true, Ordering::Relaxed);
        gtk4::glib::Propagation::Proceed
    });

    window
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
            background: #222226;
            box-shadow: none;
        }

        .clipboard-list {
            background: transparent;
        }

        .clipboard-item {
            background: #343437;
            border: 2px solid transparent;
            border-radius: 10px;
            padding: 10px 14px;
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
            font-family: monospace;
            opacity: 0.9;
        }

        .clipboard-time {
            font-size: 0.8em;
            opacity: 0.6;
        }
        "
    );

    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Show the overlay if it's hidden
pub fn show_overlay() {
    OVERLAY_WINDOW.with(|window| {
        if let Some(ref win) = *window.borrow() {
            win.set_visible(true);
            win.present();
        }
    });
}

/// Hide the overlay without closing it
pub fn hide_overlay() {
    OVERLAY_WINDOW.with(|window| {
        if let Some(ref win) = *window.borrow() {
            win.set_visible(false);
        }
    });
}

/// Create a clipboard history item row from backend data
fn create_clipboard_item_from_backend(item: &ClipboardItemPreview) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.add_css_class("clipboard-item");

    let main_box = Box::new(Orientation::Vertical, 6);
    main_box.set_margin_top(8);
    main_box.set_margin_bottom(8);
    main_box.set_margin_start(12);
    main_box.set_margin_end(12);

    // Header with content type and time
    let header_box = Box::new(Orientation::Horizontal, 8);
    
    let type_label = Label::new(Some(&item.content_preview_type.get_icon()));
    type_label.add_css_class("caption");
    
    let type_text = Label::new(Some(&capitalize_first_letter(item.content_preview_type.to_string())));
    type_text.add_css_class("caption");
    type_text.set_halign(Align::Start);
    type_text.set_hexpand(true);
    
    let time_label = Label::new(Some(&format_timestamp(item.timestamp)));
    time_label.add_css_class("caption");
    time_label.add_css_class("clipboard-time");
    time_label.set_halign(Align::End);

    header_box.append(&type_label);
    header_box.append(&type_text);
    header_box.append(&time_label);
    
    main_box.append(&header_box);

    let content_label = Label::new(Some(&item.content_preview));
    content_label.add_css_class("clipboard-preview");
    content_label.set_halign(Align::Start);
    content_label.set_wrap(true);
    content_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    content_label.set_max_width_chars(50);
    content_label.set_lines(3);
    content_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

    main_box.append(&content_label);

    row.set_child(Some(&main_box));
    row
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

/// Capitalize first letter of a string
fn capitalize_first_letter(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
