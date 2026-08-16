use lorepia_domain::{
    BuiltInTemplateValue, CapabilityKey, ConditionExpr, SafeTemplate, TemplatePart, TemplateSlot,
    VariableMap, VariableRef, VariableValue,
};
use thiserror::Error;

/// Read-only inputs available to condition and template evaluation.
#[derive(Debug, Clone, Copy)]
pub struct TemplateEnvironment<'a> {
    pub variables: &'a VariableMap,
    pub capabilities: &'a [CapabilityKey],
    pub character_name: &'a str,
    pub user_name: &'a str,
    pub persona_name: Option<&'a str>,
    pub persona_description: Option<&'a str>,
    /// Caller-supplied value. The renderer never reads a clock.
    pub current_date: &'a str,
    /// Caller-supplied value. The renderer never reads a clock.
    pub current_time: &'a str,
    pub slots: &'a [TemplateSlot],
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TemplateError {
    #[error("invalid template contract: {0}")]
    Invalid(String),
    #[error("variable `{0}` is not defined")]
    MissingVariable(String),
    #[error("variable `{0}` has an incompatible type")]
    IncompatibleVariable(String),
    #[error("template slot `{0}` is not defined")]
    MissingSlot(String),
    #[error("rendered template exceeds its {limit}-character limit")]
    OutputLimit { limit: u32 },
}

/// Evaluates the bounded condition AST without side effects.
///
/// # Errors
///
/// Returns an error if persisted input is invalid or an operation is applied
/// to an incompatible variable type.
pub fn evaluate_condition(
    expression: &ConditionExpr,
    variables: &VariableMap,
    capabilities: &[CapabilityKey],
) -> Result<bool, TemplateError> {
    expression
        .validate()
        .map_err(|error| TemplateError::Invalid(error.to_string()))?;
    evaluate_condition_validated(expression, variables, capabilities)
}

fn evaluate_condition_validated(
    expression: &ConditionExpr,
    variables: &VariableMap,
    capabilities: &[CapabilityKey],
) -> Result<bool, TemplateError> {
    match expression {
        ConditionExpr::True => Ok(true),
        ConditionExpr::False => Ok(false),
        ConditionExpr::Equals { variable, value } => {
            Ok(require_variable(variables, variable)? == value)
        }
        ConditionExpr::NotEquals { variable, value } => {
            Ok(require_variable(variables, variable)? != value)
        }
        ConditionExpr::GreaterThan { variable, value } => {
            let actual = require_variable(variables, variable)?;
            let actual = match actual {
                VariableValue::Integer(value) => {
                    // Conditions intentionally compare the two numeric
                    // variants in a common domain. Persisted finite bounds are
                    // validated before this explicit lossy conversion.
                    #[allow(clippy::cast_precision_loss)]
                    let numeric = *value as f64;
                    numeric
                }
                VariableValue::Decimal(value) => *value,
                _ => return Err(incompatible(variable)),
            };
            Ok(actual > *value)
        }
        ConditionExpr::Contains { variable, value } => {
            let actual = require_variable(variables, variable)?;
            match actual {
                VariableValue::Text(actual) | VariableValue::Enum(actual) => {
                    Ok(actual.contains(value))
                }
                VariableValue::StringList(actual) => Ok(actual.contains(value)),
                _ => Err(incompatible(variable)),
            }
        }
        ConditionExpr::Exists { variable } => Ok(variables.get(variable).is_some()),
        ConditionExpr::ModelSupports { capability } => Ok(capabilities.contains(capability)),
        ConditionExpr::All { expressions } => {
            for expression in expressions {
                if !evaluate_condition_validated(expression, variables, capabilities)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ConditionExpr::Any { expressions } => {
            for expression in expressions {
                if evaluate_condition_validated(expression, variables, capabilities)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ConditionExpr::Not { expression } => Ok(!evaluate_condition_validated(
            expression,
            variables,
            capabilities,
        )?),
    }
}

/// Renders a value-insertion-only template. It cannot mutate state, execute
/// code, read files, access a network, or evaluate regular expressions.
///
/// # Errors
///
/// Returns an error for invalid templates, missing inputs, type mismatches, or
/// output that exceeds the persisted bound.
pub fn render_safe_template(
    template: &SafeTemplate,
    environment: &TemplateEnvironment<'_>,
) -> Result<String, TemplateError> {
    template
        .validate()
        .map_err(|error| TemplateError::Invalid(error.to_string()))?;
    render_validated(template, environment)
}

fn render_validated(
    template: &SafeTemplate,
    environment: &TemplateEnvironment<'_>,
) -> Result<String, TemplateError> {
    let mut output = String::new();
    let mut output_chars = 0_usize;
    let limit = usize::try_from(template.max_output_chars).unwrap_or(usize::MAX);

    for part in &template.parts {
        match part {
            TemplatePart::Text { value } => {
                append_bounded(&mut output, &mut output_chars, value, limit)?;
            }
            TemplatePart::Variable { variable } => {
                let value = format_variable(require_variable(environment.variables, variable)?);
                append_bounded(&mut output, &mut output_chars, &value, limit)?;
            }
            TemplatePart::BuiltIn { value } => {
                let value = built_in_value(*value, environment);
                append_bounded(&mut output, &mut output_chars, value, limit)?;
            }
            TemplatePart::Slot { name } => {
                let value = environment
                    .slots
                    .iter()
                    .find(|slot| slot.name == *name)
                    .map(|slot| slot.value.as_str())
                    .ok_or_else(|| TemplateError::MissingSlot(name.clone()))?;
                append_bounded(&mut output, &mut output_chars, value, limit)?;
            }
            TemplatePart::Join {
                variable,
                separator,
            } => {
                let VariableValue::StringList(values) =
                    require_variable(environment.variables, variable)?
                else {
                    return Err(incompatible(variable));
                };
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        append_bounded(&mut output, &mut output_chars, separator, limit)?;
                    }
                    append_bounded(&mut output, &mut output_chars, value, limit)?;
                }
            }
            TemplatePart::Conditional {
                condition,
                then_template,
                else_template,
            } => {
                let selected = if evaluate_condition_validated(
                    condition,
                    environment.variables,
                    environment.capabilities,
                )? {
                    Some(then_template.as_ref())
                } else {
                    else_template.as_deref()
                };
                if let Some(selected) = selected {
                    let rendered = render_validated(selected, environment)?;
                    append_bounded(&mut output, &mut output_chars, &rendered, limit)?;
                }
            }
        }
    }
    Ok(output)
}

fn append_bounded(
    output: &mut String,
    output_chars: &mut usize,
    value: &str,
    limit: usize,
) -> Result<(), TemplateError> {
    let appended_chars = value.chars().count();
    let Some(next_count) = output_chars.checked_add(appended_chars) else {
        return Err(TemplateError::OutputLimit {
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
        });
    };
    if next_count > limit {
        return Err(TemplateError::OutputLimit {
            limit: u32::try_from(limit).unwrap_or(u32::MAX),
        });
    }
    output.push_str(value);
    *output_chars = next_count;
    Ok(())
}

fn require_variable<'a>(
    variables: &'a VariableMap,
    variable: &VariableRef,
) -> Result<&'a VariableValue, TemplateError> {
    variables
        .get(variable)
        .ok_or_else(|| TemplateError::MissingVariable(variable_label(variable)))
}

fn incompatible(variable: &VariableRef) -> TemplateError {
    TemplateError::IncompatibleVariable(variable_label(variable))
}

fn variable_label(variable: &VariableRef) -> String {
    variable.namespace.as_ref().map_or_else(
        || variable.id.0.clone(),
        |namespace| format!("{}.{}", namespace.0, variable.id.0),
    )
}

fn format_variable(value: &VariableValue) -> String {
    match value {
        VariableValue::Bool(value) => value.to_string(),
        VariableValue::Integer(value) => value.to_string(),
        VariableValue::Decimal(value) => value.to_string(),
        VariableValue::Text(value) | VariableValue::Enum(value) => value.clone(),
        VariableValue::StringList(values) => values.join(", "),
    }
}

fn built_in_value<'a>(
    value: BuiltInTemplateValue,
    environment: &'a TemplateEnvironment<'a>,
) -> &'a str {
    match value {
        BuiltInTemplateValue::CharacterName => environment.character_name,
        BuiltInTemplateValue::UserName => environment.user_name,
        BuiltInTemplateValue::PersonaName => environment.persona_name.unwrap_or(""),
        BuiltInTemplateValue::PersonaDescription => environment.persona_description.unwrap_or(""),
        BuiltInTemplateValue::CurrentDate => environment.current_date,
        BuiltInTemplateValue::CurrentTime => environment.current_time,
    }
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        ConditionExpr, SafeTemplate, TemplatePart, VariableBinding, VariableId, VariableMap,
        VariableRef, VariableScope, VariableValue,
    };

    use super::{TemplateEnvironment, TemplateError, evaluate_condition, render_safe_template};

    fn variable() -> VariableRef {
        VariableRef {
            scope: VariableScope::Conversation,
            namespace: None,
            id: VariableId::from("mood"),
        }
    }

    fn environment(variables: &VariableMap) -> TemplateEnvironment<'_> {
        TemplateEnvironment {
            variables,
            capabilities: &[],
            character_name: "Ari",
            user_name: "Sam",
            persona_name: None,
            persona_description: None,
            current_date: "2026-08-03",
            current_time: "12:00",
            slots: &[],
        }
    }

    #[test]
    fn condition_and_template_use_typed_read_only_values() {
        let variable = variable();
        let variables = VariableMap {
            values: vec![VariableBinding {
                variable: variable.clone(),
                value: VariableValue::Text("bright".into()),
            }],
        };
        let condition = ConditionExpr::Equals {
            variable: variable.clone(),
            value: VariableValue::Text("bright".into()),
        };
        let template = SafeTemplate {
            parts: vec![
                TemplatePart::Text {
                    value: "Mood: ".into(),
                },
                TemplatePart::Variable { variable },
                TemplatePart::Conditional {
                    condition: condition.clone(),
                    then_template: Box::new(SafeTemplate {
                        parts: vec![TemplatePart::Text { value: "!".into() }],
                        max_output_chars: 8,
                    }),
                    else_template: None,
                },
            ],
            max_output_chars: 32,
        };

        assert!(evaluate_condition(&condition, &variables, &[]).expect("condition"));
        assert_eq!(
            render_safe_template(&template, &environment(&variables)).expect("render"),
            "Mood: bright!"
        );
    }

    #[test]
    fn missing_values_and_output_overflow_are_explicit() {
        let variables = VariableMap::default();
        let missing = SafeTemplate {
            parts: vec![TemplatePart::Variable {
                variable: variable(),
            }],
            max_output_chars: 8,
        };
        assert!(matches!(
            render_safe_template(&missing, &environment(&variables)),
            Err(TemplateError::MissingVariable(_))
        ));

        let oversized = SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: "12345".into(),
            }],
            max_output_chars: 4,
        };
        assert_eq!(
            render_safe_template(&oversized, &environment(&variables)),
            Err(TemplateError::OutputLimit { limit: 4 })
        );
    }
}
