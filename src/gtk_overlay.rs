use gtk4::prelude::*;
use gtk4::{Application, Button, Label, Box, Orientation, Align};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::RefCell;

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

/// Create a Windows 11-style clipboard history list with regular libadwaita styling
fn create_overlay_content() -> Box {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar with title
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Clipboard History"))));
    header_bar.set_show_end_title_buttons(false); // Disable default close button
    header_bar.set_show_start_title_buttons(false); // Disable default buttons on the left too

    // Add close button to header (upper right corner)
    let close_button = Button::new();
    close_button.set_icon_name("window-close-symbolic");
    close_button.add_css_class("circular");
    header_bar.pack_end(&close_button);
    
    // Add clear all button to header
    let clear_button = Button::with_label("Clear All");
    clear_button.add_css_class("destructive-action");
    header_bar.pack_end(&clear_button);

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

    // Sample clipboard items (in a real app, these would come from actual clipboard history)
    let clipboard_items = vec![
        ("Hello, world!", "text", "2 minutes ago"),
        ("https://github.com/rust-lang/rust", "url", "5 minutes ago"),
        ("impl Display for MyStruct {\n    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {\n        write!(f, \"MyStruct\")\n    }\n}", "code", "10 minutes ago"),
        ("Password123!", "password", "15 minutes ago"),
        ("Meeting notes:\n- Discuss project timeline\n- Review code changes\n- Plan next sprint", "text", "1 hour ago"),
        ("cursor-clip-v1.0.0.tar.gz", "file", "2 hours ago"),
    ];

    for (content, content_type, time) in clipboard_items {
        let row = create_clipboard_item(content, content_type, time);
        list_box.append(&row);
    }

    // Handle item selection
    list_box.connect_row_selected(|_, row| {
        if let Some(row) = row {
            println!("Selected clipboard item: {:?}", row.index());
            // Here you would copy the selected item to clipboard
        }
    });

    scrolled_window.set_child(Some(&list_box));
    main_box.append(&scrolled_window);


    //let close_button = Button::with_label("Close");
    //close_button.add_css_class("suggested-action");
//
    //footer_box.append(&close_button);
    //main_box.append(&footer_box);

    // Connect button signals
    clear_button.connect_clicked(move |_| {
        println!("Clear all clipboard history");
        // Here you would clear the clipboard history
    });

    close_button.connect_clicked(move |_| {
        println!("Close button clicked - closing both overlay and capture layer");
        CLOSE_REQUESTED.store(true, Ordering::Relaxed);
        // Don't set GTK_CLOSE_ONLY to true - we want to close everything
        
        OVERLAY_WINDOW.with(|window| {
            if let Some(ref win) = *window.borrow() {
                win.close();
            }
        });
    });

    //close_button.connect_clicked(move |_| {
    //    println!("Close clipboard history window");
    //    GTK_CLOSE_ONLY.store(true, Ordering::Relaxed);
    //    CLOSE_REQUESTED.store(true, Ordering::Relaxed);
    //    
    //    OVERLAY_WINDOW.with(|window| {
    //        if let Some(ref win) = *window.borrow() {
    //            win.close();
    //        }
    //    });
    //});

    main_box
}

/// Main entry point for creating the overlay
pub fn create_clipboard_overlay(x: f64, y: f64) {
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
}

/// Create and configure the layer shell window
fn create_layer_shell_window(app: &Application, x: f64, y: f64) -> adw::ApplicationWindow {
    // Create the main window using Adwaita ApplicationWindow
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Clipboard History")
        .decorated(false)  // Disable window decorations to avoid duplicate close buttons
        //.default_width(600)
        //.default_height(300)
        //.resizable(true)
        .build();

    // Initialize layer shell for this window
    window.init_layer_shell();

    // Configure layer shell properties
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("cursor-clip"));

    // Anchor to top-left corner for precise positioning
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    //window.set_anchor(Edge::Bottom, false);
    //window.set_anchor(Edge::Right, false);
    
    // Set margins to position the window at the specified coordinates
    window.set_margin(Edge::Top, y as i32);
    window.set_margin(Edge::Left, x as i32);
    
    //-1 means no exclusive zone, allowing clicks through
    // 0 means exclusive zone is the size of the window -> moves the window down
    window.set_exclusive_zone(-1); 

    // Make window keyboard interactive
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::OnDemand);

    // Apply custom styling
    apply_custom_styling(&window);

    // Create and set content
    let content = create_overlay_content();
    window.set_content(Some(&content));

    // Add focus-out handler to close when clicking outside
    window.connect_is_active_notify(|window| {
        println!("inside GTK window focus handler (checking outsite)");
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
