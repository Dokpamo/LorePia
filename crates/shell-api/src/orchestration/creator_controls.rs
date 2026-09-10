use std::collections::BTreeMap;

use lorepia_core::{
    ControlKind, ControlSpec, CreatorControlValue as CoreCreatorControlValue, VariableValue,
};

use super::{CreatorControlProjectionDto, MAX_SELECTION_ITEMS, ShellResult, shell_invalid};

pub(super) fn project_creator_controls(
    controls: &[ControlSpec],
    values: &BTreeMap<String, CoreCreatorControlValue>,
) -> ShellResult<Vec<CreatorControlProjectionDto>> {
    if controls.len() > MAX_SELECTION_ITEMS {
        return Err(shell_invalid(
            "prompt preset exceeds the creator control projection limit",
        ));
    }
    controls
        .iter()
        .filter(|control| {
            !control.sensitive
                && !matches!(
                    control.kind,
                    ControlKind::Section | ControlKind::Caption | ControlKind::Divider
                )
        })
        .map(|control| {
            let value = values
                .get(control.id.as_str())
                .cloned()
                .or_else(|| {
                    control
                        .default_value
                        .as_ref()
                        .and_then(variable_to_creator_control_value)
                })
                .ok_or_else(|| shell_invalid("interactive creator control has no safe value"))?;
            let choices = control
                .options
                .iter()
                .map(|option| match &option.value {
                    VariableValue::Text(value) | VariableValue::Enum(value) => Ok(value.clone()),
                    _ => Err(shell_invalid(
                        "select creator control has a non-text option value",
                    )),
                })
                .collect::<ShellResult<Vec<_>>>()?;
            let choice_labels = control
                .options
                .iter()
                .map(|option| option.label.clone())
                .collect();
            Ok(CreatorControlProjectionDto {
                id: control.id.as_str().to_owned(),
                label: control.label.clone(),
                description: (!control.description.is_empty()).then(|| control.description.clone()),
                kind: control.kind,
                value,
                choices,
                choice_labels,
                minimum: control.minimum,
                maximum: control.maximum,
                step: control.step,
            })
        })
        .collect()
}

fn variable_to_creator_control_value(value: &VariableValue) -> Option<CoreCreatorControlValue> {
    match value {
        VariableValue::Bool(value) => Some(CoreCreatorControlValue::Bool(*value)),
        VariableValue::Integer(value) => Some(CoreCreatorControlValue::Integer(*value)),
        VariableValue::Decimal(value) if value.is_finite() => {
            Some(CoreCreatorControlValue::Decimal(*value))
        }
        VariableValue::Text(value) | VariableValue::Enum(value) => {
            Some(CoreCreatorControlValue::Text(value.clone()))
        }
        VariableValue::StringList(values) => {
            Some(CoreCreatorControlValue::StringList(values.clone()))
        }
        VariableValue::Decimal(_) => None,
    }
}
