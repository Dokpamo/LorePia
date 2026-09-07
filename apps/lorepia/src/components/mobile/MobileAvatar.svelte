<script lang="ts">
    import CharacterAvatar from '../../features/assets/CharacterAvatar.svelte';
    import type { CharacterDto, LorepiaClient } from '../../lib/ipc/contracts';

    let { client, character }: { client: LorepiaClient; character: CharacterDto | null } = $props();
    function toneFor(id: string): number {
        let sum = 0;
        for (const character of id) sum += character.charCodeAt(0);
        return sum % 4;
    }
    const tone = $derived(toneFor(character?.id ?? ''));
</script>

<span class="mobile-avatar" data-tone={tone}>
    <CharacterAvatar {client} {character} />
</span>
