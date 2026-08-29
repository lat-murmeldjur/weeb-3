use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};

use crate::{bzz_stream::BzzMetadata, events::ProgressRow};

pub(crate) fn property(value: &JsValue, name: &str) -> JsValue {
    Reflect::get(value, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

pub(crate) fn set(target: &Object, name: &str, value: JsValue) {
    let _ = Reflect::set(target, &JsValue::from_str(name), &value);
}

pub(crate) fn set_string(target: &Object, name: &str, value: impl AsRef<str>) {
    set(target, name, JsValue::from_str(value.as_ref()));
}

pub(crate) fn set_number(target: &Object, name: &str, value: f64) {
    set(target, name, JsValue::from_f64(value));
}

pub(crate) fn set_bool(target: &Object, name: &str, value: bool) {
    set(target, name, JsValue::from_bool(value));
}

pub(crate) fn string_property(value: &JsValue, name: &str) -> Option<String> {
    property(value, name).as_string()
}

pub(crate) fn bool_property(value: &JsValue, name: &str) -> Option<bool> {
    property(value, name).as_bool()
}

pub(crate) fn number_property(value: &JsValue, name: &str) -> Option<f64> {
    property(value, name)
        .as_f64()
        .filter(|value| value.is_finite())
}

pub(crate) fn integer_property(value: &JsValue, name: &str) -> Option<u64> {
    let value = number_property(value, name)?;
    (value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64).then_some(value as u64)
}

pub(crate) fn array_property(value: &JsValue, name: &str) -> Option<Array> {
    property(value, name).dyn_into().ok()
}

pub(crate) fn bytes_to_js(bytes: &[u8]) -> Uint8Array {
    let value = Uint8Array::new_with_length(bytes.len() as u32);
    value.copy_from(bytes);
    value
}

pub(crate) fn bytes_from_js(value: &JsValue, name: &str) -> Option<Vec<u8>> {
    property(value, name)
        .dyn_into::<Uint8Array>()
        .ok()
        .map(|value| value.to_vec())
}

pub(crate) fn metadata_to_js(metadata: &BzzMetadata) -> Object {
    let value = Object::new();
    set(
        &value,
        "dataReference",
        bytes_to_js(&metadata.data_reference).into(),
    );
    set_string(&value, "mime", &metadata.mime);
    set_number(&value, "size", metadata.size as f64);
    set_string(&value, "etag", &metadata.etag);
    set_string(&value, "path", &metadata.path);
    set_number(&value, "targetCount", metadata.target_count as f64);
    value
}

pub(crate) fn metadata_from_js(value: &JsValue) -> Option<BzzMetadata> {
    let data_reference = bytes_from_js(value, "dataReference")?;
    if data_reference.is_empty() {
        return None;
    }
    Some(BzzMetadata {
        data_reference,
        mime: string_property(value, "mime")?,
        size: integer_property(value, "size")?,
        etag: string_property(value, "etag")?,
        path: string_property(value, "path")?,
        target_count: integer_property(value, "targetCount")?.try_into().ok()?,
    })
}

pub(crate) fn progress_to_js(row: ProgressRow) -> Object {
    let object = Object::new();
    set_string(&object, "id", &row.id);
    set_string(&object, "kind", &row.kind);
    set_string(&object, "subject", &row.subject);
    set_string(&object, "phase", &row.phase);
    set_optional_percent(&object, row.percent);
    set_string(&object, "detail", &row.detail);
    set_bool(&object, "done", row.done);
    set_bool(&object, "ok", row.ok);
    object
}

pub(crate) fn progress_from_js(value: &JsValue) -> Option<ProgressRow> {
    let percent = integer_property(value, "percent")
        .filter(|value| *value <= 100)
        .and_then(|value| u8::try_from(value).ok());
    Some(ProgressRow {
        id: string_property(value, "id")?,
        kind: string_property(value, "kind")?,
        subject: string_property(value, "subject")?,
        phase: string_property(value, "phase")?,
        percent,
        detail: string_property(value, "detail")?,
        done: bool_property(value, "done")?,
        ok: bool_property(value, "ok")?,
    })
}

pub(crate) fn set_optional_percent(target: &Object, percent: Option<u8>) {
    let value = percent.map_or(JsValue::NULL, |value| JsValue::from_f64(value.into()));
    set(target, "percent", value);
}
