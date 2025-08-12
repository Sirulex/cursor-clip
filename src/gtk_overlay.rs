use gtk4::prelude::*;
use gtk4::{Application, Button, Label, Box, Orientation, Align};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use libadwaita::{self as adw, prelude::*};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::cell::RefCell;

static INIT: Once = Once::new();
pub static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);
pub static GTK_CLOSE_ONLY: AtomicBool = AtomicBool::new(false);

// Thread-local storage for the overlay state since GTK objects aren't Send/Sync
thread_local! {
    static OVERLAY_WINDOW: RefCell<Option<adw::ApplicationWindow>> = RefCell::new(None);
    static OVERLAY_APP: RefCell<Option<Application>> = RefCell::new(None);
}

pub fn is_close_requested() -> bool {
    CLOSE_REQUESTED.load(Ordering::Relaxed)
}

pub fn is_gtk_close_only() -> bool {
    GTK_CLOSE_ONLY.load(Ordering::Relaxed)
}

pub fn reset_close_flags() {
    CLOSE_REQUESTED.store(false, Ordering::Relaxed);
    GTK_CLOSE_ONLY.store(false, Ordering::Relaxed);
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

/// Create the main overlay window content
fn create_overlay_content() -> Box {
    // You can choose between simple and enhanced content
    // For demonstration purposes, let's use the enhanced version
    create_enhanced_content_simple()
}

/// Create a Windows 11-style clipboard history list with regular libadwaita styling
fn create_enhanced_content_simple() -> Box {
    // Main container with standard libadwaita spacing
    let main_box = Box::new(Orientation::Vertical, 0);

    // Header bar with title
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Clipboard History"))));
    
    // Add clear all button to header
    let clear_button = Button::with_label("Clear All");
    clear_button.add_css_class("destructive-action");
    header_bar.pack_end(&clear_button);

    main_box.append(&header_bar);

    // Create scrolled window for the clipboard list
    let scrolled_window = gtk4::ScrolledWindow::new();
    scrolled_window.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled_window.set_min_content_height(300);
    scrolled_window.set_min_content_width(400);

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

    // Footer with action buttons
    let footer_box = Box::new(Orientation::Horizontal, 12);
    footer_box.set_margin_top(12);
    footer_box.set_margin_bottom(12);
    footer_box.set_margin_start(12);
    footer_box.set_margin_end(12);
    footer_box.set_halign(Align::End);

    let close_button = Button::with_label("Close");
    close_button.add_css_class("suggested-action");

    footer_box.append(&close_button);
    main_box.append(&footer_box);

    // Connect button signals
    clear_button.connect_clicked(move |_| {
        println!("Clear all clipboard history");
        // Here you would clear the clipboard history
    });

    close_button.connect_clicked(move |_| {
        println!("Close clipboard history window");
        GTK_CLOSE_ONLY.store(true, Ordering::Relaxed);
        CLOSE_REQUESTED.store(true, Ordering::Relaxed);
        
        OVERLAY_WINDOW.with(|window| {
            if let Some(ref win) = *window.borrow() {
                win.close();
            }
        });
    });

    main_box
}

/// Create and configure the layer shell window
fn create_layer_shell_window_impl(app: &Application, x: f64, y: f64) -> adw::ApplicationWindow {
    // Create the main window using Adwaita ApplicationWindow
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Clipboard History")
        .default_width(450)
        .default_height(400)
        .resizable(true)
        .build();

    // Initialize layer shell for this window
    window.init_layer_shell();

    // Configure layer shell properties
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("cursor-clip"));

    // Anchor to top-left corner for precise positioning
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Bottom, false);
    window.set_anchor(Edge::Right, false);
    
    // Set margins to position the window at the specified coordinates
    window.set_margin(Edge::Top, y as i32);
    window.set_margin(Edge::Left, x as i32);
    
    // Don't reserve space on the desktop
    window.set_exclusive_zone(0);

    // Make window keyboard interactive
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::OnDemand);

    // Apply custom styling
    apply_custom_styling(&window);

    // Create and set content
    let content = create_overlay_content();
    window.set_content(Some(&content));

    window
}

/// Apply minimal custom CSS styling for the clipboard list
fn apply_custom_styling(window: &adw::ApplicationWindow) {
    let css_provider = gtk4::CssProvider::new();
    css_provider.load_from_data(
        "
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

/// Create additional content widgets using libadwaita components
fn create_enhanced_content() -> Box {
    let main_container = Box::new(Orientation::Vertical, 0);
    
    // Create a modern header bar with libadwaita styling
    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&Label::new(Some("Cursor Clip"))));
    
    // Add menu button to header
    let menu_button = Button::from_icon_name("open-menu-symbolic");
    menu_button.add_css_class("flat");
    header_bar.pack_end(&menu_button);
    
    // Create a preferences group for settings
    let preferences_page = adw::PreferencesPage::new();
    
    // Main settings group
    let settings_group = adw::PreferencesGroup::new();
    settings_group.set_title("Settings");
    settings_group.set_description(Some("Configure clipboard behavior"));
    
    // Create some action rows
    let auto_copy_row = adw::ActionRow::new();
    auto_copy_row.set_title("Auto Copy");
    auto_copy_row.set_subtitle("Automatically copy selected text");
    
    let auto_copy_switch = gtk4::Switch::new();
    auto_copy_switch.set_active(true);
    auto_copy_row.add_suffix(&auto_copy_switch);
    
    let history_row = adw::ActionRow::new();
    history_row.set_title("Clipboard History");
    history_row.set_subtitle("Enable clipboard history tracking");
    
    let history_switch = gtk4::Switch::new();
    history_switch.set_active(false);
    history_row.add_suffix(&history_switch);
    
    // Create a simple row for history size
    let history_size_row = adw::ActionRow::new();
    history_size_row.set_title("History Size");
    history_size_row.set_subtitle("Number of items to keep in history");
    
    let history_size_entry = gtk4::Entry::new();
    history_size_entry.set_text("10");
    history_size_entry.set_input_purpose(gtk4::InputPurpose::Number);
    history_size_entry.set_width_chars(5);
    history_size_entry.set_valign(Align::Center);
    history_size_row.add_suffix(&history_size_entry);
    
    settings_group.add(&auto_copy_row);
    settings_group.add(&history_row);
    settings_group.add(&history_size_row);
    
    // Create a second group for appearance
    let appearance_group = adw::PreferencesGroup::new();
    appearance_group.set_title("Appearance");
    
    let theme_row = adw::ComboRow::new();
    theme_row.set_title("Theme");
    theme_row.set_subtitle("Choose application theme");
    
    let theme_model = gtk4::StringList::new(&["Auto", "Light", "Dark"]);
    theme_row.set_model(Some(&theme_model));
    
    appearance_group.add(&theme_row);
    
    preferences_page.add(&settings_group);
    preferences_page.add(&appearance_group);
    
    // Create action buttons group
    let action_group = adw::PreferencesGroup::new();
    action_group.set_title("Actions");
    
    let clear_history_row = adw::ActionRow::new();
    clear_history_row.set_title("Clear History");
    clear_history_row.set_subtitle("Remove all clipboard history");
    
    let clear_button = Button::with_label("Clear");
    clear_button.add_css_class("destructive-action");
    clear_button.set_valign(Align::Center);
    clear_history_row.add_suffix(&clear_button);
    
    action_group.add(&clear_history_row);
    preferences_page.add(&action_group);
    
    // Add everything to main container
    main_container.append(&header_bar);
    main_container.append(&preferences_page);
    
    // Connect signals
    clear_button.connect_clicked(move |_| {
        println!("Clear history button clicked");
        // Here you would implement actual history clearing
    });
    
    auto_copy_switch.connect_state_set(|_, state| {
        println!("Auto copy toggled: {}", state);
        gtk4::glib::Propagation::Proceed
    });
    
    history_switch.connect_state_set(|_, state| {
        println!("History tracking toggled: {}", state);
        gtk4::glib::Propagation::Proceed
    });
    
    theme_row.connect_selected_notify(|row| {
        let selected = row.selected();
        let theme = match selected {
            0 => "Auto",
            1 => "Light", 
            2 => "Dark",
            _ => "Auto",
        };
        println!("Theme changed to: {}", theme);
        
        // Apply theme change
        let style_manager = adw::StyleManager::default();
        match selected {
            1 => style_manager.set_color_scheme(adw::ColorScheme::ForceLight),
            2 => style_manager.set_color_scheme(adw::ColorScheme::ForceDark),
            _ => style_manager.set_color_scheme(adw::ColorScheme::Default),
        }
    });
    
    main_container
}

/// Create a toast overlay for notifications
fn create_toast_overlay(child: &impl IsA<gtk4::Widget>) -> adw::ToastOverlay {
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(child));
    toast_overlay
}

/// Show a toast notification
pub fn show_toast(message: &str) {
    // This would need to be called from the main thread with access to the toast overlay
    println!("Toast: {}", message);
}

/// Main entry point for creating the overlay
pub fn create_layer_shell_window(x: f64, y: f64) {
    let app = init_application();
    
    let app_clone = app.clone();
    app.connect_activate(move |_| {
        let window = create_layer_shell_window_impl(&app_clone, x, y);
        
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
