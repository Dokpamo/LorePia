<script lang="ts">
    import { Bot, Brain, Globe, HardDrive, Monitor, Puzzle, SlidersHorizontal, UserRound } from '@lucide/svelte';
    import type { SettingsSection } from '../settings-contracts';
    let { section }: { section: SettingsSection } = $props();
    const icons = { ai: Bot, persona: UserRound, prompt: SlidersHorizontal, memory: Brain, appearance: Monitor, language: Globe, plugins: Puzzle, storage: HardDrive };
    const Icon = $derived(section in icons ? icons[section as keyof typeof icons] : SlidersHorizontal);
</script>

<Icon aria-hidden="true" />
