use js_sys::{Function, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Document, Element, Event, HtmlButtonElement, HtmlElement};

const INTERFACE_STYLE_ID: &str = "weeb3InterfaceStyle";

const INDEX_HTML: &str = include_str!("../static/index.html");

fn interface_style() -> &'static str {
    let start = r#"<style id="weeb3InterfaceStyle">"#;
    let Some(start_index) = INDEX_HTML.find(start).map(|index| index + start.len()) else {
        return "";
    };
    let Some(end_index) = INDEX_HTML[start_index..]
        .find("</style>")
        .map(|index| start_index + index)
    else {
        return "";
    };

    INDEX_HTML[start_index..end_index].trim()
}

fn interface_shell() -> &'static str {
    let start = r#"<div id="wrapper">"#;
    let Some(start_index) = INDEX_HTML.find(start) else {
        return "";
    };
    let after_start = &INDEX_HTML[start_index..];
    let Some(end_index) = after_start.find("<script>") else {
        return after_start.trim();
    };

    after_start[..end_index].trim()
}

fn wallet_helper_script() -> &'static str {
    let start = "<script>";
    let Some(start_index) = INDEX_HTML.find(start).map(|index| index + start.len()) else {
        return "";
    };
    let Some(end_index) = INDEX_HTML[start_index..]
        .find("</script>")
        .map(|index| start_index + index)
    else {
        return "";
    };

    INDEX_HTML[start_index..end_index].trim()
}
#[derive(Clone, Copy)]
struct CollapseSection {
    button_id: &'static str,
    panel_id: &'static str,
    title: &'static str,
}

const PRIMARY_SECTIONS: &[CollapseSection] = &[
    CollapseSection {
        button_id: "settingsToggle",
        panel_id: "settingsPanel",
        title: "Settings",
    },
    CollapseSection {
        button_id: "navigationToggle",
        panel_id: "navigationPanel",
        title: "Navigation",
    },
    CollapseSection {
        button_id: "storageToggle",
        panel_id: "storagePanel",
        title: "Create Storage on Swarm",
    },
    CollapseSection {
        button_id: "uploadToggle",
        panel_id: "uploadPanel",
        title: "Upload",
    },
    CollapseSection {
        button_id: "bandwidthToggle",
        panel_id: "bandwidthPanel",
        title: "Bandwidth",
    },
];

const LOG_SECTIONS: &[CollapseSection] = &[CollapseSection {
    button_id: "logsToggle",
    panel_id: "logsPanel",
    title: "Logs",
}];

pub(crate) fn install_interface_conventions() {
    install_collapsible_group(PRIMARY_SECTIONS);
    install_collapsible_group(LOG_SECTIONS);
    set_section_state(LOG_SECTIONS[0], true);
    install_static_button_labels();
    install_theme_toggle();
}

pub(crate) fn render_interface_shell(container: &Element) {
    ensure_style();
    ensure_wallet_helper();
    container.set_inner_html(interface_shell());
}

fn document() -> Option<Document> {
    web_sys::window()?.document()
}

fn element(id: &str) -> Option<Element> {
    document()?.get_element_by_id(id)
}

fn ensure_style() {
    let Some(document) = document() else {
        return;
    };
    if let Some(style) = document.get_element_by_id(INTERFACE_STYLE_ID) {
        style.set_text_content(Some(interface_style()));
        return;
    }
    let Ok(style) = document.create_element("style") else {
        return;
    };
    style.set_id(INTERFACE_STYLE_ID);
    style.set_text_content(Some(interface_style()));
    if let Some(body) = document.body() {
        let _ = body.append_child(&style);
    }
}

fn ensure_wallet_helper() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let key = JsValue::from_str("weeb3EnsureEip1193");
    if Reflect::get(&window, &key)
        .ok()
        .and_then(|value| value.dyn_into::<Function>().ok())
        .is_some()
    {
        return;
    }

    let source = wallet_helper_script();
    if source.is_empty() {
        return;
    }

    let Ok(_) = Function::new_no_args(source).call0(&JsValue::NULL) else {
        return;
    };
}

fn html_element(id: &str) -> Option<HtmlElement> {
    element(id)?.dyn_into::<HtmlElement>().ok()
}

pub(crate) fn set_bracket_button_label(button: &Element, label: &str) {
    button.set_text_content(Some(label));
}

fn set_menu_button_label(button: &Element, label: &str) {
    button.set_text_content(Some(&format!("[ {} ]", label)));
}

fn set_button_label_by_id(button_id: &str, label: &str) {
    if let Some(button) = element(button_id) {
        set_bracket_button_label(&button, label);
    }
}

fn install_static_button_labels() {
    for (button_id, label) in [
        ("networkSet", " Change network settings "),
        ("transferPauseToggle", " Pause retrieve / push "),
        ("uploadGetBatch", " Create Storage on Swarm for Uploads "),
        ("uploadResetStamp", " Reuse Space on Swarm for New Uploads "),
        ("uploadFile", " Upload on Swarm "),
        ("deployChequebook", " Deploy chequebook "),
        ("depositCash", " Deposit cash "),
    ] {
        set_button_label_by_id(button_id, label);
    }
}

fn panel_open(panel_id: &str) -> bool {
    element(panel_id)
        .map(|panel| !panel.has_attribute("hidden"))
        .unwrap_or(false)
}

fn set_section_state(section: CollapseSection, open: bool) {
    if let Some(panel) = element(section.panel_id) {
        if open {
            let _ = panel.remove_attribute("hidden");
        } else {
            let _ = panel.set_attribute("hidden", "");
        }
        let _ = panel.set_attribute("aria-hidden", if open { "false" } else { "true" });
    }

    if let Some(button) = html_element(section.button_id) {
        set_menu_button_label(
            button.unchecked_ref::<Element>(),
            &format!("{} {}", section.title, if open { "-" } else { "+" }),
        );
    }
}

fn install_collapsible_group(sections: &'static [CollapseSection]) {
    for section in sections {
        set_section_state(*section, false);

        let Some(button) = element(section.button_id) else {
            continue;
        };
        let Some(button) = button.dyn_ref::<HtmlButtonElement>() else {
            continue;
        };
        let current = *section;
        let callback = Closure::<dyn FnMut(Event)>::new(move |_event| {
            let next_open = !panel_open(current.panel_id);
            for sibling in sections {
                set_section_state(*sibling, next_open && sibling.panel_id == current.panel_id);
            }
        });

        button.set_onclick(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
    }
}

fn install_theme_toggle() {
    let Some(button) = element("themeToggle") else {
        return;
    };
    let Some(button) = button.dyn_ref::<HtmlButtonElement>() else {
        return;
    };

    set_theme_button_text();
    install_os_theme_listener();

    let callback = Closure::<dyn FnMut(Event)>::new(move |_event| {
        let Some(document) = document() else {
            return;
        };
        let Some(body) = document.body() else {
            return;
        };

        if effective_dark() {
            let _ = body.set_attribute("class", "light");
        } else {
            let _ = body.set_attribute("class", "dark");
        }

        set_theme_button_text();
    });

    button.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
}

fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-color-scheme: dark)")
                .ok()
                .flatten()
        })
        .map(|query| query.matches())
        .unwrap_or(false)
}

fn theme_class(name: &str) -> bool {
    document()
        .and_then(|document| document.body())
        .and_then(|body| body.get_attribute("class"))
        .map(|class_name| {
            class_name
                .split_whitespace()
                .any(|class_item| class_item == name)
        })
        .unwrap_or(false)
}

fn effective_dark() -> bool {
    theme_class("dark") || (!theme_class("light") && prefers_dark())
}

fn set_theme_button_text() {
    if let Some(button) = element("themeToggle") {
        let label = if effective_dark() {
            "Light Mode"
        } else {
            "Dark Mode"
        };
        set_menu_button_label(&button, label);
    }
}

fn install_os_theme_listener() {
    let Some(query) = web_sys::window().and_then(|window| {
        window
            .match_media("(prefers-color-scheme: dark)")
            .ok()
            .flatten()
    }) else {
        return;
    };
    let Some(target) = query.dyn_ref::<web_sys::EventTarget>() else {
        return;
    };

    let callback = Closure::<dyn FnMut(Event)>::new(move |_event| {
        set_theme_button_text();
    });

    let _ = target.add_event_listener_with_callback("change", callback.as_ref().unchecked_ref());
    callback.forget();
}
