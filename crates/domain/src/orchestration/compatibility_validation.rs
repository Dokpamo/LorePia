use super::{
    ContentCapability, ContentModule, MAX_IDENTIFIER_CHARS, MAX_MODULE_COMPONENTS, MAX_NAME_CHARS,
    MemoryProfile, OrchestrationValidationError, PlacementZone, PromptBlockKind, TemplatePart,
    ValidateOrchestration, validate_control, validate_id, validate_prompt_block,
    validate_provenance, validate_text,
};

impl ValidateOrchestration for MemoryProfile {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        validate_id("summary_task", self.summary_task.as_str())?;
        validate_id("summary_schema", self.summary_schema.as_str())?;
        if !self
            .summary_schema
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
        {
            return Err(OrchestrationValidationError::new(
                "summary_schema",
                "must contain only canonical ASCII identifier characters",
            ));
        }
        if let Some(embedding_task) = &self.embedding_task {
            validate_id("embedding_task", embedding_task.as_str())?;
        }
        let retrieval_weights = [
            self.recency_weight,
            self.similarity_weight,
            self.importance_weight,
        ];
        if self.schema_version == 0
            || self.turns_per_summary == 0
            || self.turns_per_summary > 10_000
            || self.retrieval_count == 0
            || self.retrieval_count > 10_000
            || self.recent_raw_budget.max_tokens > 10_000_000
            || self.episodic_budget.max_tokens > 10_000_000
            || self.semantic_budget.max_tokens > 10_000_000
            || retrieval_weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.0)
        {
            return Err(OrchestrationValidationError::new(
                "memory_profile",
                "schema, counts, and finite non-negative weights are required",
            ));
        }
        if !retrieval_weights.iter().any(|weight| *weight > 0.0) {
            return Err(OrchestrationValidationError::new(
                "retrieval_weights",
                "at least one retrieval weight must be positive",
            ));
        }
        if let Some(template) = &self.summary_template {
            template.validate().map_err(|error| {
                OrchestrationValidationError::new(
                    format!("summary_template.{}", error.path),
                    error.reason,
                )
            })?;
            let mut source_slots = 0_usize;
            for (index, part) in template.parts.iter().enumerate() {
                match part {
                    TemplatePart::Text { .. } => {}
                    TemplatePart::Slot { name } if name == "memory_source" => {
                        source_slots = source_slots.saturating_add(1);
                    }
                    TemplatePart::Slot { .. } => {
                        return Err(OrchestrationValidationError::new(
                            format!("summary_template.parts[{index}].name"),
                            "only the memory_source slot is available",
                        ));
                    }
                    _ => {
                        return Err(OrchestrationValidationError::new(
                            format!("summary_template.parts[{index}]"),
                            "memory summary templates allow only literal text and one memory_source slot",
                        ));
                    }
                }
            }
            if source_slots != 1 {
                return Err(OrchestrationValidationError::new(
                    "summary_template",
                    "must contain exactly one memory_source slot",
                ));
            }
        }
        validate_provenance(&self.provenance, "provenance")
    }
}

impl ValidateOrchestration for ContentModule {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        validate_text("version", &self.version, 1, MAX_IDENTIFIER_CHARS)?;
        if self.schema_version == 0 {
            return Err(OrchestrationValidationError::new(
                "schema_version",
                "must be positive",
            ));
        }
        let component_count = self.prompt_fragments.len()
            + self.knowledge_book_ids.len()
            + self.control_specs.len()
            + self.transform_set_ids.len()
            + self.interaction_rule_set_ids.len()
            + self.asset_ids.len();
        if component_count > MAX_MODULE_COMPONENTS {
            return Err(OrchestrationValidationError::new(
                "components",
                format!("must contain at most {MAX_MODULE_COMPONENTS} components"),
            ));
        }
        validate_text(
            "metadata.license",
            &self.metadata.license,
            1,
            MAX_NAME_CHARS,
        )?;
        validate_text(
            "metadata.description",
            &self.metadata.description,
            0,
            16_384,
        )?;
        validate_provenance(&self.metadata.provenance, "metadata.provenance")?;
        let mut block_ids = Vec::with_capacity(self.prompt_fragments.len());
        for (index, block) in self.prompt_fragments.iter().enumerate() {
            validate_prompt_block(block, &format!("prompt_fragments[{index}]"))?;
            if block.kind == PromptBlockKind::LatestUserTurn
                || block.placement_zone == PlacementZone::ApplicationPolicy
            {
                return Err(OrchestrationValidationError::new(
                    format!("prompt_fragments[{index}]"),
                    "modules cannot replace fixed application or latest-user blocks",
                ));
            }
            block_ids.push(&block.id);
        }
        block_ids.sort();
        if block_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "prompt_fragments",
                "block identifiers must be unique",
            ));
        }
        for (index, control) in self.control_specs.iter().enumerate() {
            validate_control(control, &format!("control_specs[{index}]"))?;
        }
        let mut capabilities = self.required_capabilities.clone();
        capabilities.sort();
        if capabilities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "required_capabilities",
                "capabilities must be unique",
            ));
        }
        if self.portable_runtime.is_some()
            != capabilities.contains(&ContentCapability::PortableRuntime)
        {
            return Err(OrchestrationValidationError::new(
                "portable_runtime",
                "portable runtime content requires exactly one matching capability declaration",
            ));
        }
        Ok(())
    }
}
