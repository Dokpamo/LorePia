<script lang="ts">
    import './runtime.css';
    import { SvelteSet } from 'svelte/reactivity';

    import { tr } from '../../../lib/i18n';
    import TrustedAsset from '../../../features/assets/TrustedAsset.svelte';
    import type {
        InteractionRoomCapableClient,
        InteractionRoomController,
        InteractionRoomState,
        RoomInteractionEffect,
    } from '../../../features/chat/interaction-room-controller';

    interface Props {
        client: InteractionRoomCapableClient;
        controller: InteractionRoomController;
        state: InteractionRoomState;
    }

    let { client, controller, state }: Props = $props();

    const displayEffects = $derived.by(() => {
        const retained: RoomInteractionEffect[] = [];
        const visualRegions = new SvelteSet<string>();
        for (const effect of [...state.effects].reverse()) {
            if (
                effect.effect.kind === 'state_changed' ||
                effect.effect.kind === 'knowledge_activated'
            ) {
                continue;
            }
            if (effect.effect.kind === 'show_asset') {
                if (visualRegions.has(effect.effect.region)) continue;
                visualRegions.add(effect.effect.region);
            }
            retained.push(effect);
            if (retained.length >= 32) break;
        }
        return retained.reverse();
    });
</script>

{#if state.phase === 'loading'}
    <div role="status">{$tr('interaction.surface.loading')}</div>
{:else if state.error === null && state.announcement !== ''}
    <div role="status" aria-live="polite">
        {state.announcement}
    </div>
{/if}

{#if state.error !== null}<p role="alert">{state.error}</p>{/if}

{#if state.has_more_expired_proposals}
    <div role="alert">
        <span>
            {$tr('interaction.surface.expired_more')}
        </span>
        <button
            class="ui-submit ui-pressable"
            type="button"
            disabled={state.phase === 'loading'}
            onclick={() => void controller.reload()}
            ><span class="ui-press-visual">
                {$tr('interaction.surface.expired_more_action')}
            </span></button
        >
    </div>
{/if}

{#if displayEffects.length > 0 || state.pending_proposals.length > 0}
    <section class="ui-settings-group" aria-labelledby="room-interaction-title">
        <header>
            <h3 class="ui-settings-heading" id="room-interaction-title">
                {$tr('interaction.surface.title')}
            </h3>
            <span>
                {$tr('interaction.surface.revision', {
                    revision: state.current_state_revision,
                })}
            </span>
        </header>

        {#if displayEffects.length > 0}
            <ul class="ui-runtime-list">
                {#each displayEffects as interactionEffect (interactionEffect.effect_id)}
                    <li>
                        {#if interactionEffect.effect.kind === 'show_asset'}
                            <p>
                                {$tr('interaction.surface.asset_label', {
                                    region: interactionEffect.effect.region,
                                })}
                            </p>
                            <div>
                                <TrustedAsset
                                    {client}
                                    selector={{
                                        kind: 'asset_id',
                                        asset_id: interactionEffect.effect.asset.asset_id,
                                    }}
                                    expectedKind={interactionEffect.effect.asset.kind}
                                    alt={$tr('interaction.surface.asset_alt', {
                                        region: interactionEffect.effect.region,
                                    })}
                                    showMetadata
                                />
                            </div>
                        {:else if interactionEffect.effect.kind === 'play_audio'}
                            <p>{$tr('interaction.surface.audio')}</p>
                            <div>
                                <TrustedAsset
                                    {client}
                                    selector={{
                                        kind: 'asset_id',
                                        asset_id: interactionEffect.effect.asset.asset_id,
                                    }}
                                    expectedKind="audio"
                                    alt={$tr('interaction.surface.audio')}
                                    showMetadata
                                />
                            </div>
                        {:else if interactionEffect.effect.kind === 'present_choices'}
                            <fieldset>
                                <legend>{$tr('interaction.surface.choices')}</legend>
                                <div>
                                    {#each interactionEffect.effect.choices as choice (choice.id)}
                                        <button
                                            class="ui-submit ui-pressable"
                                            type="button"
                                            class:ui-runtime-selected={interactionEffect.selected_choice_id ===
                                                choice.id}
                                            disabled={interactionEffect.choice_status !==
                                                'pending' ||
                                                state.busy_effect_id ===
                                                    interactionEffect.effect_id}
                                            onclick={() =>
                                                void controller.submitChoice(
                                                    interactionEffect.effect_id,
                                                    choice.id,
                                                )}
                                            ><span class="ui-press-visual">
                                                {choice.label}
                                            </span></button
                                        >
                                    {/each}
                                </div>
                                {#if interactionEffect.choice_status === 'consumed'}
                                    <p>
                                        {$tr('interaction.surface.choice_selected', {
                                            choice:
                                                interactionEffect.selected_choice_id ??
                                                $tr('interaction.surface.unknown'),
                                        })}
                                    </p>
                                {:else if interactionEffect.choice_status === 'expired'}
                                    <p>
                                        {$tr('interaction.surface.choice_expired')}
                                    </p>
                                {/if}
                            </fieldset>
                        {:else if interactionEffect.effect.kind === 'visible_system_event'}
                            <p>{interactionEffect.effect.text}</p>
                        {:else if interactionEffect.effect.kind === 'dice_rolled'}
                            <p>
                                {$tr('interaction.surface.dice', {
                                    count: interactionEffect.effect.count,
                                    sides: interactionEffect.effect.sides,
                                    modifier: `${interactionEffect.effect.modifier >= 0 ? '+' : ''}${String(interactionEffect.effect.modifier)}`,
                                    rolls: interactionEffect.effect.rolls.join(', '),
                                    total: interactionEffect.effect.total,
                                })}
                            </p>
                        {:else if interactionEffect.effect.kind === 'approval_pending'}
                            <article class="ui-runtime-item">
                                <h4 class="ui-settings-heading">
                                    {interactionEffect.effect.title}
                                </h4>
                                <p>{interactionEffect.effect.body}</p>
                                {#if interactionEffect.effect.expires_after_seconds !== null}
                                    <small>
                                        {$tr('interaction.surface.approval_expires', {
                                            seconds: interactionEffect.effect.expires_after_seconds,
                                        })}
                                    </small>
                                {/if}
                            </article>
                        {:else if interactionEffect.effect.kind === 'projection_rejected'}
                            <p role="status">
                                {interactionEffect.effect.reason === 'asset_unavailable'
                                    ? $tr('interaction.surface.projection.asset_unavailable')
                                    : interactionEffect.effect.reason === 'unsafe_native_text'
                                      ? $tr('interaction.surface.projection.unsafe_text')
                                      : $tr('interaction.surface.projection.incompatible')}
                            </p>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}

        {#if state.pending_proposals.length > 0}
            <section aria-labelledby="interaction-proposals-title">
                <h4 class="ui-settings-heading" id="interaction-proposals-title">
                    {$tr('interaction.surface.proposals')}
                </h4>
                <ul class="ui-runtime-list">
                    {#each state.pending_proposals as item (item.proposal.id)}
                        <li>
                            {#if item.proposal.projection_rejection_reason === 'unsafe_native_text'}
                                <strong>{$tr('attempt_approval.unrenderable.title')}</strong>
                                <p>{$tr('attempt_approval.unrenderable.hint')}</p>
                            {:else}
                                <strong>{item.proposal.title}</strong>
                                <p>{item.proposal.body}</p>
                            {/if}
                            <div>
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    disabled={state.busy_proposal_id !== null ||
                                        state.has_more_expired_proposals}
                                    onclick={() =>
                                        void controller.decideProposal(item.proposal.id, 'reject')}
                                    ><span class="ui-press-visual">
                                        {$tr('attempt_approval.reject')}
                                    </span></button
                                >
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    disabled={state.busy_proposal_id !== null ||
                                        state.has_more_expired_proposals ||
                                        item.proposal.projection_rejection_reason ===
                                            'unsafe_native_text'}
                                    onclick={() =>
                                        void controller.decideProposal(item.proposal.id, 'approve')}
                                    ><span class="ui-press-visual">
                                        {$tr('attempt_approval.approve')}
                                    </span></button
                                >
                            </div>
                        </li>
                    {/each}
                </ul>
            </section>
        {/if}

        {#if state.has_older_effects}
            <p>
                {$tr('interaction.surface.older_effects')}
            </p>
        {/if}
    </section>
{/if}
