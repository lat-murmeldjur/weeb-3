use wasm_bindgen::JsValue;
use web_sys::window;

pub async fn clear_path() {
    let window = window().unwrap();
    let location = window.location();
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("/#/") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 3..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("/#") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 2..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("#/") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 2..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    let _ = match location.href() {
        Ok(href0) => {
            if let Some(hash0) = href0.find("#") {
                let p0 = &href0[..hash0];
                let p1 = &href0[hash0 + 1..];

                let new_url = format!("{}/{}", p0, p1);
                match window.history() {
                    Ok(history) => {
                        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                    }
                    _ => {}
                };
            }
        }
        _ => {}
    };
    loop {
        let _ = match location.pathname() {
            Ok(path0) => {
                if let Some(slashslash) = path0.find("//") {
                    let p0 = &path0[..slashslash];
                    let p1 = &path0[slashslash + 2..];

                    let origin = location.origin().unwrap();
                    let new_url = format!("{}{}/{}", origin, p0, p1);
                    match window.history() {
                        Ok(history) => {
                            let _ =
                                history.replace_state_with_url(&JsValue::NULL, "", Some(&new_url));
                        }
                        _ => {
                            break;
                        }
                    };
                } else {
                    break;
                }
            }
            _ => {
                break;
            }
        };
    }
}
