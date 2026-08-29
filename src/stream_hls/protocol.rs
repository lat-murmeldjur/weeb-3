use js_sys::Object;
use wasm_bindgen::JsValue;

use crate::{
    stream_hls::HlsStartupPlan,
    worker_protocol::{bool_property, number_property, set, set_number},
};

pub(crate) fn plan_to_js(plan: &HlsStartupPlan) -> Object {
    let object = Object::new();
    set_number(&object, "bootstrapPosition", plan.bootstrap_position);
    set(
        &object,
        "codecBootstrap",
        JsValue::from_bool(plan.codec_bootstrap),
    );
    set_number(&object, "playPosition", plan.play_position);
    set_number(&object, "runwayEnd", plan.runway_end);
    set_number(&object, "duration", plan.duration);
    object
}

pub(crate) fn plan_from_js(value: &JsValue) -> Option<HlsStartupPlan> {
    let plan = HlsStartupPlan {
        bootstrap_position: number_property(value, "bootstrapPosition")?,
        codec_bootstrap: bool_property(value, "codecBootstrap")?,
        play_position: number_property(value, "playPosition")?,
        runway_end: number_property(value, "runwayEnd")?,
        duration: number_property(value, "duration")?,
    };
    (plan.bootstrap_position >= 0.0
        && plan.play_position >= 0.0
        && plan.runway_end > plan.play_position
        && plan.duration >= plan.runway_end
        && plan.bootstrap_position <= plan.play_position)
        .then_some(plan)
}
