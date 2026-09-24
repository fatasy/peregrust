//! Native gamepad snapshots, polled explicitly by JavaScript.
//!
//! Gilrs applies SDL-compatible controller mappings. We convert its named
//! controls to the Web Gamepad standard layout and never retain pressed state
//! for a device that is absent from the current connected-gamepad list.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::time::Instant;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use gilrs::{Axis, Button, Gamepad, GamepadId, Gilrs};
use serde::Serialize;

const MAX_GAMEPADS: usize = 32;
const MAX_EVENTS_PER_POLL: usize = 4096;

// https://w3c.github.io/gamepad/#remapping
const STANDARD_BUTTONS: [Button; 17] = [
    Button::South,         // 0: bottom face (A / Cross)
    Button::East,          // 1: right face (B / Circle)
    Button::West,          // 2: left face (X / Square)
    Button::North,         // 3: top face (Y / Triangle)
    Button::LeftTrigger,   // 4: left shoulder
    Button::RightTrigger,  // 5: right shoulder
    Button::LeftTrigger2,  // 6: left analog trigger
    Button::RightTrigger2, // 7: right analog trigger
    Button::Select,        // 8: back/select
    Button::Start,         // 9: start
    Button::LeftThumb,     // 10: left stick press
    Button::RightThumb,    // 11: right stick press
    Button::DPadUp,        // 12
    Button::DPadDown,      // 13
    Button::DPadLeft,      // 14
    Button::DPadRight,     // 15
    Button::Mode,          // 16: center/home
];

const STANDARD_AXES: [Axis; 4] = [
    Axis::LeftStickX,
    Axis::LeftStickY,
    Axis::RightStickX,
    Axis::RightStickY,
];

#[derive(Clone, Serialize)]
struct GamepadButtonSnapshot {
    pressed: bool,
    touched: bool,
    value: f32,
}

#[derive(Clone, Serialize)]
struct GamepadSnapshot {
    id: String,
    index: usize,
    connected: bool,
    mapping: &'static str,
    axes: [f32; 4],
    buttons: [GamepadButtonSnapshot; 17],
    timestamp: f64,
}

struct GamepadService {
    gilrs: Option<Gilrs>,
    init_error: Option<String>,
    slots: Vec<GamepadId>,
    indices: HashMap<GamepadId, usize>,
    started: Instant,
}

impl GamepadService {
    fn new() -> Self {
        Self {
            gilrs: None,
            init_error: None,
            slots: Vec::new(),
            indices: HashMap::new(),
            started: Instant::now(),
        }
    }

    fn poll(&mut self) -> Result<Vec<Option<GamepadSnapshot>>, JsErrorBox> {
        if let Some(error) = &self.init_error {
            return Err(JsErrorBox::generic(error.clone()));
        }
        if self.gilrs.is_none() {
            match Gilrs::new() {
                Ok(gilrs) => self.gilrs = Some(gilrs),
                Err(error) => {
                    let message = format!("cannot initialize native gamepads: {error}");
                    self.init_error = Some(message.clone());
                    return Err(JsErrorBox::generic(message));
                }
            }
        }
        let gilrs = self.gilrs.as_mut().expect("initialized above");
        // Gilrs updates cached button/axis state while yielding events. Bound
        // work per JS call so noisy devices cannot monopolize the frame loop.
        for _ in 0..MAX_EVENTS_PER_POLL {
            if gilrs.next_event().is_none() {
                break;
            }
        }

        let timestamp = self.started.elapsed().as_secs_f64() * 1000.0;
        let connected: Vec<(GamepadId, GamepadSnapshot)> = gilrs
            .gamepads()
            .take(MAX_GAMEPADS + 1)
            .map(|(id, pad)| (id, snapshot(&pad, timestamp)))
            .collect();
        if connected.len() > MAX_GAMEPADS {
            return Err(JsErrorBox::range_error(
                "more than 32 gamepads are connected",
            ));
        }
        let connected_ids: HashSet<GamepadId> = connected.iter().map(|(id, _)| *id).collect();
        let mut result: Vec<Option<GamepadSnapshot>> = std::iter::repeat_with(|| None)
            .take(self.slots.len())
            .collect();
        for (id, mut pad) in connected {
            let index = assign_slot(
                &mut self.slots,
                &mut self.indices,
                id,
                &connected_ids,
                MAX_GAMEPADS,
            )
            .ok_or_else(|| JsErrorBox::range_error("no free gamepad slot"))?;
            if index >= result.len() {
                result.resize(index + 1, None);
            }
            pad.index = index;
            result[index] = Some(pad);
        }
        Ok(result)
    }
}

fn snapshot(pad: &Gamepad<'_>, timestamp: f64) -> GamepadSnapshot {
    let axes = STANDARD_AXES.map(|axis| {
        normalize_axis(
            pad.value(axis),
            matches!(axis, Axis::LeftStickY | Axis::RightStickY),
        )
    });
    let buttons = STANDARD_BUTTONS.map(|button| match pad.button_data(button) {
        Some(data) => normalize_button(data.value(), data.is_pressed()),
        None => normalize_button(
            if pad.is_pressed(button) { 1.0 } else { 0.0 },
            pad.is_pressed(button),
        ),
    });
    let standard = [
        Button::South,
        Button::East,
        Button::West,
        Button::North,
        Button::LeftTrigger,
        Button::RightTrigger,
        Button::LeftTrigger2,
        Button::RightTrigger2,
        Button::Select,
        Button::Start,
        Button::DPadUp,
        Button::DPadDown,
        Button::DPadLeft,
        Button::DPadRight,
    ]
    .iter()
    .all(|button| pad.button_code(*button).is_some())
        && STANDARD_AXES
            .iter()
            .all(|axis| pad.axis_code(*axis).is_some());
    GamepadSnapshot {
        id: pad.name().to_owned(),
        index: 0, // assigned from the stable slot table below
        connected: true,
        mapping: if standard { "standard" } else { "" },
        axes,
        buttons,
        timestamp,
    }
}

fn normalize_axis(value: f32, invert_y: bool) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    let value = value.clamp(-1.0, 1.0);
    if invert_y { -value } else { value }
}

fn normalize_button(value: f32, pressed: bool) -> GamepadButtonSnapshot {
    let mut value = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if pressed && value == 0.0 {
        value = 1.0;
    }
    GamepadButtonSnapshot {
        pressed: pressed || value >= 0.5,
        touched: pressed || value > 0.0,
        value,
    }
}

fn assign_slot<Id: Copy + Eq + Hash>(
    slots: &mut Vec<Id>,
    indices: &mut HashMap<Id, usize>,
    id: Id,
    connected: &HashSet<Id>,
    limit: usize,
) -> Option<usize> {
    if let Some(index) = indices.get(&id) {
        return Some(*index);
    }
    if slots.len() < limit {
        let index = slots.len();
        slots.push(id);
        indices.insert(id, index);
        return Some(index);
    }
    let index = slots.iter().position(|old| !connected.contains(old))?;
    indices.remove(&slots[index]);
    slots[index] = id;
    indices.insert(id, index);
    Some(index)
}

#[op2]
#[serde]
fn op_peregrust_gamepad_poll(
    state: &mut OpState,
) -> Result<Vec<Option<GamepadSnapshot>>, JsErrorBox> {
    state.borrow_mut::<GamepadService>().poll()
}

deno_core::extension!(
    peregrust_gamepad,
    ops = [op_peregrust_gamepad_poll],
    state = |state| state.put(GamepadService::new()),
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_axes_and_analog_buttons_are_normalized() {
        assert_eq!(normalize_axis(0.75, false), 0.75);
        assert_eq!(normalize_axis(0.75, true), -0.75);
        assert_eq!(normalize_axis(f32::NAN, false), 0.0);
        assert_eq!(normalize_axis(9.0, false), 1.0);
        let trigger = normalize_button(0.7, false);
        assert!(trigger.pressed && trigger.touched);
        assert_eq!(trigger.value, 0.7);
        assert!(!normalize_button(0.0, false).touched);
    }

    #[test]
    fn disconnected_slot_is_empty_and_can_be_reused_without_stuck_state() {
        let mut slots = Vec::new();
        let mut indices = HashMap::new();
        let first = HashSet::from([10usize, 20]);
        assert_eq!(
            assign_slot(&mut slots, &mut indices, 10, &first, 2),
            Some(0)
        );
        assert_eq!(
            assign_slot(&mut slots, &mut indices, 20, &first, 2),
            Some(1)
        );
        let after_disconnect = HashSet::from([20usize, 30]);
        assert_eq!(
            assign_slot(&mut slots, &mut indices, 30, &after_disconnect, 2),
            Some(0)
        );
        assert!(!indices.contains_key(&10));
        assert_eq!(indices.get(&20), Some(&1));
        assert_eq!(indices.get(&30), Some(&0));
    }

    #[test]
    fn face_and_trigger_order_matches_standard_layout() {
        assert_eq!(STANDARD_BUTTONS[0], Button::South);
        assert_eq!(STANDARD_BUTTONS[1], Button::East);
        assert_eq!(STANDARD_BUTTONS[2], Button::West);
        assert_eq!(STANDARD_BUTTONS[3], Button::North);
        assert_eq!(STANDARD_BUTTONS[6], Button::LeftTrigger2);
        assert_eq!(STANDARD_BUTTONS[7], Button::RightTrigger2);
        assert_eq!(
            STANDARD_AXES,
            [
                Axis::LeftStickX,
                Axis::LeftStickY,
                Axis::RightStickX,
                Axis::RightStickY
            ]
        );
    }
}
