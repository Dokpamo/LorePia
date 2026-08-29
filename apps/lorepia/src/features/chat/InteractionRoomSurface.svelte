<script lang="ts">
    import { SvelteSet } from 'svelte/reactivity';

    import { tr } from '../../lib/i18n';
    import TrustedAsset from '../assets/TrustedAsset.svelte';
    import type {
        InteractionRoomCapableClient,
        InteractionRoomController,
        InteractionRoomState,
        RoomInteractionEffect,
    } from './interaction-room-controller';

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
    <div class="interaction-status" role="status">{$tr('interaction.surface.loading')}</div>
{:else if state.error === null && state.announcement !== ''}
    <div class="interaction-status" role="status" aria-live="polite">
        {state.announcement}
    </div>
{/if}

{#if state.has_more_expired_proposals}
    <div class="interaction-status error" role="alert">
        <span>
            {$tr('interaction.surface.expired_more')}
        </span>
        <button
            type="button"
            disabled={state.phase === 'loading'}
            onclick={() => void controller.reload()}
        >
            {$tr('interaction.surface.expired_more_action')}
        </button>
    </div>
{/if}

{#if displayEffects.length > 0 || state.pending_proposals.length > 0}
    <section class="interaction-surface" aria-labelledby="room-interaction-title">
        <header>
            <h3 id="room-interaction-title">{$tr('interaction.surface.title')}</h3>
            <span>
                {$tr('interaction.surface.revision', {
                    revision: state.current_state_revision,
                })}
            </span>
        </header>

        {#if displayEffects.length > 0}
            <ul class="interaction-effects">
                {#each displayEffects as interactionEffect (interactionEffect.effect_id)}
                    <li>
                        {#if interactionEffect.effect.kind === 'show_asset'}
                            <p class="interaction-label">
                                {$tr('interaction.surface.asset_label', {
                                    region: interactionEffect.effect.region,
                                })}
                            </p>
                            <div class="interaction-media">
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
                            <p class="interaction-label">{$tr('interaction.surface.audio')}</p>
                            <div class="interaction-audio">
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
                                <div class="interaction-actions">
                                    {#each interactionEffect.effect.choices as choice (choice.id)}
                                        <button
                                            type="button"
                                            class:primary={interactionEffect.selected_choice_id ===
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
                                        >
                                            {choice.label}
                                        </button>
                                    {/each}
                                </div>
                                {#if interactionEffect.choice_status === 'consumed'}
                                    <p class="interaction-label">
                                        {$tr('interaction.surface.choice_selected', {
                                            choice:
                                                interactionEffect.selected_choice_id ??
                                                $tr('interaction.surface.unknown'),
                                        })}
                                    </p>
                                {:else if interactionEffect.choice_status === 'expired'}
                                    <p class="interaction-label">
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
                            <article>
                                <h4>{interactionEffect.effect.title}</h4>
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
                            <p class="interaction-label" role="status">
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
                <h4 id="interaction-proposals-title">{$tr('interaction.surface.proposals')}</h4>
                <ul class="interaction-proposals">
                    {#each state.pending_proposals as item (item.proposal.id)}
                        <li>
                            {#if item.proposal.projection_rejection_reason === 'unsafe_native_text'}
                                <strong>{$tr('attempt_approval.unrenderable.title')}</strong>
                                <p>{$tr('attempt_approval.unrenderable.hint')}</p>
                            {:else}
                                <strong>{item.proposal.title}</strong>
                                <p>{item.proposal.body}</p>
                            {/if}
                            <div class="interaction-actions">
                                <button
                                    type="button"
                                    disabled={state.busy_proposal_id !== null ||
                                        state.has_more_expired_proposals}
                                    onclick={() =>
                                        void controller.decideProposal(item.proposal.id, 'reject')}
                                >
                                    {$tr('attempt_approval.reject')}
                                </button>
                                <button
                                    class="primary"
                                    type="button"
                                    disabled={state.busy_proposal_id !== null ||
                                        state.has_more_expired_proposals ||
                                        item.proposal.projection_rejection_reason ===
                                            'unsafe_native_text'}
                                    onclick={() =>
                                        void controller.decideProposal(item.proposal.id, 'approve')}
                                >
                                    {$tr('attempt_approval.approve')}
                                </button>
                            </div>
                        </li>
                    {/each}
                </ul>
            </section>
        {/if}

        {#if state.has_older_effects}
            <p class="interaction-label">
                {$tr('interaction.surface.older_effects')}
            </p>
        {/if}
    </section>
{/if}

<style>
    .interaction-status,
    .interaction-surface {
        width: min(100% - 2 * clamp(16px, 5vw, 32px), var(--reading));
        margin: 8px auto 0;
    }

    .interaction-status {
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    .interaction-surface {
        max-height: min(42vh, 32rem);
        padding: 12px;
        overflow-y: auto;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface-sunken);
    }

    .interaction-surface > header,
    .interaction-actions {
        display: flex;
        gap: 8px;
        align-items: center;
        justify-content: space-between;
    }

    .interaction-surface h3,
    .interaction-surface h4,
    .interaction-surface p {
        margin: 0;
    }

    .interaction-surface > header span,
    .interaction-label {
        color: var(--ink-muted);
        font-size: 0.72rem;
    }

    .interaction-effects,
    .interaction-proposals {
        display: grid;
        gap: 8px;
        margin: 10px 0 0;
        padding: 0;
        list-style: none;
    }

    .interaction-effects > li,
    .interaction-proposals > li {
        display: grid;
        gap: 8px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--surface);
    }

    .interaction-media {
        width: min(100%, 28rem);
        height: min(38vh, 20rem);
        overflow: hidden;
        border-radius: 10px;
    }

    .interaction-audio {
        width: min(100%, 32rem);
        min-height: 3rem;
    }

    .interaction-actions {
        flex-wrap: wrap;
        justify-content: flex-start;
    }
</style>
