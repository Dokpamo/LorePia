
fn legacy_builtin_prompt_preset(mut preset: PromptPreset) -> PromptPreset {
    if let Some(story_instruction) = preset
        .blocks
        .iter_mut()
        .find(|block| block.id.as_str().ends_with(".story-instruction"))
    {
        story_instruction.authority = lorepia_domain::InstructionAuthority::Application;
    }
    let history_index = preset
        .blocks
        .iter()
        .position(|block| block.kind == PromptBlockKind::HistorySlice)
        .expect("built-in history block");
    let history = preset.blocks.remove(history_index);
    let post_history_index = preset
        .blocks
        .iter()
        .position(|block| block.kind == PromptBlockKind::PostHistoryInstruction)
        .expect("built-in post-history block");
    preset.blocks.insert(post_history_index + 1, history);
    preset
}

#[test]
fn built_in_prompt_presets_have_canonical_placement_order() {
    for preset in built_in_prompt_presets() {
        preset
            .validate()
            .expect("built-in compatibility preset must satisfy the prompt contract");
        assert!(
            preset
                .blocks
                .windows(2)
                .all(|pair| pair[0].placement_zone <= pair[1].placement_zone)
        );
    }
}
