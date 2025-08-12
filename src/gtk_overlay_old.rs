use gtk4::prelude::*;
use gtk4::{Application, Button, Label, Box, Orientation, Align};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use std::sync::{Once, Arc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::RefCell;
use tokio::sync::Mutex;
use crate::frontend::FrontendClient;
use crate::ipc::ClipboardItem;

use gtk4::prelude::*;
use gtk4::{Application, Button, Label, Box, Orientation, Align};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use std::sync::{Once, Arc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::RefCell;
use tokio::sync::Mutex;
use crate::frontend::FrontendClient;
use crate::ipc::ClipboardItem;

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
fn create_overlay_content(client: Arc<Mutex<FrontendClient>>) -> Box {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar with title
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Clipboard History"))));
    header_bar.set_show_end_title_buttons(false);
    header_bar.set_show_start_title_buttons(false);

    // Add close button to header (upper right corner)
    let close_button = Button::new();
    close_button.set_icon_name("window-close-symbolic");
    close_button.add_css_class("circular");
    header_bar.pack_end(&close_button);
    
    // Add clear all button to header
    let clear_button = Button::with_label("Clear All");
    clear_button.add_css_class("destructive-action");
    header_bar.pack_start(&clear_button);

    main_box.append(&header_bar);

    // Create scrolled window for the clipboard list
    let scrolled_window = gtk4::ScrolledWindow::new();
    scrolled_window.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled_window.set_min_content_width(200);
    scrolled_window.set_min_content_height(300);

    // Create list box for clipboard items
    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::Single);

    // Load clipboard items from backend
    let rt = tokio::runtime::Runtime::new().unwrap();
    let items = rt.block_on(async {
        let mut client_lock = client.lock().await;
        client_lock.get_history().await.unwrap_or_else(|e| {
            eprintln!("Error getting clipboard history: {}", e);
            Vec::new()
        })
    });

    // Populate list with items from backend
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
    let client_for_selection = client.clone();
    list_box.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let index = row.index() as usize;
            if index < items.len() {
                let content = items[index].content.clone();
                println!("Selected clipboard item: {}", content);
                
                // Set clipboard content via backend
                let client_clone = client_for_selection.clone();
                tokio::spawn(async move {
                    let mut client_lock = client_clone.lock().await;
                    if let Err(e) = client_lock.set_clipboard(content).await {
                        eprintln!("Error setting clipboard: {}", e);
                    }
                });
            }
        }
    });

    scrolled_window.set_child(Some(&list_box));
    main_box.append(&scrolled_window);

    // Connect button signals
    let client_for_clear = client.clone();
    clear_button.connect_clicked(move |_| {
        println!("Clear all clipboard history");
        let client_clone = client_for_clear.clone();
        tokio::spawn(async move {
            let mut client_lock = client_clone.lock().await;
            if let Err(e) = client_lock.clear_history().await {
                eprintln!("Error clearing clipboard history: {}", e);
            }
        });
    });

    close_button.connect_clicked(move |_| {
        println!("Close button clicked - closing both overlay and capture layer");
        CLOSE_REQUESTED.store(true, Ordering::Relaxed);
        
        OVERLAY_WINDOW.with(|window| {
            if let Some(ref win) = *window.borrow() {
                win.close();
            }
        });
    });

    main_box
}

/// Async version of the main entry point for creating the overlay
pub async fn create_clipboard_overlay_async(
    x: f64, 
    y: f64, 
    client: Arc<Mutex<FrontendClient>>
) -> Result<(), Box<dyn std::error::Error>> {
    let app = init_application();
    
    let app_clone = app.clone();
    let client_clone = client.clone();
    app.connect_activate(move |_| {
        let window = create_layer_shell_window(&app_clone, x, y, client_clone.clone());
        
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

/// Legacy sync version for compatibility
pub fn create_clipboard_overlay(x: f64, y: f64) {
    // For the sync version, we'll create a dummy client that shows placeholder data
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = rt.block_on(async {
        // Try to connect to backend, if it fails, we'll show placeholder data
        match FrontendClient::new().await {
            Ok(client) => Arc::new(Mutex::new(client)),
            Err(_) => {
                eprintln!("Could not connect to backend, showing placeholder data");
                // Create a dummy client - this won't work but prevents crashes
                return None;
            }
        }
    });

    if let Some(client) = client {
        rt.block_on(async {
            let _ = create_clipboard_overlay_async(x, y, client).await;
        });
    }
}

/// Create and configure the layer shell window
fn create_layer_shell_window(
    app: &Application, 
    x: f64, 
    y: f64, 
    client: Arc<Mutex<FrontendClient>>
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
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::OnDemand);

    // Apply custom styling
    apply_custom_styling(&window);

    // Create and set content
    let content = create_overlay_content(client);
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
        /* Modern GNOME-style rounded window */
        window {
            border-radius: 12px;
            box-shadow: 0 8px 24px rgba(0, 0, 0, 0.15);
        }
        
        /* Ensure the window content also respects the rounded corners */
        window > box {
            border-radius: 12px;
        }
        
        /* Header bar rounded corners */
        headerbar {
            border-top-left-radius: 12px;
            border-top-right-radius: 12px;
        }
        
        /* Last child element rounded bottom corners */
        window > box > scrolledwindow {
            border-bottom-left-radius: 12px;
            border-bottom-right-radius: 12px;
        }
        
        /* Minimal styling for clipboard items */
        .clipboard-item {
            padding: 8px 12px;
            margin: 2px 0;
            border-radius: 6px;
        }
        
        .clipboard-item:hover {
            background: alpha(@accent_color, 0.1);
        }
        
        .clipboard-item:selected {
            background: @accent_color;
            color: @accent_fg_color;
        }
        
        .clipboard-preview {
            font-family: monospace;
            opacity: 0.7;
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

/// Create a clipboard history item row similar to Windows 11 style
fn create_clipboard_item(content: &str, content_type: &str, time: &str) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.add_css_class("clipboard-item");

    let main_box = Box::new(Orientation::Vertical, 6);
    main_box.set_margin_top(8);
    main_box.set_margin_bottom(8);
    main_box.set_margin_start(12);
    main_box.set_margin_end(12);

    // Header with content type and time
    let header_box = Box::new(Orientation::Horizontal, 8);
    
    let type_label = Label::new(Some(&format!("{}", get_content_type_icon(content_type))));
    type_label.add_css_class("caption");
    
    let type_text = Label::new(Some(&capitalize_first_letter(content_type)));
    type_text.add_css_class("caption");
    type_text.set_halign(Align::Start);
    type_text.set_hexpand(true);
    
    let time_label = Label::new(Some(time));
    time_label.add_css_class("caption");
    time_label.add_css_class("clipboard-time");
    time_label.set_halign(Align::End);

    header_box.append(&type_label);
    header_box.append(&type_text);
    header_box.append(&time_label);
    
    main_box.append(&header_box);

    // Content preview
    let preview_text = if content.len() > 100 {
        format!("{}...", &content[..97])
    } else {
        content.to_string()
    };

    let content_label = Label::new(Some(&preview_text));
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

/// Get icon for content type
fn get_content_type_icon(content_type: &str) -> &str {
    match content_type {
        "text" => "📝",
        "url" => "🔗", 
        "code" => "💻",
        "password" => "🔒",
        "file" => "📁",
        "image" => "🖼️",
        _ => "📄",
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
