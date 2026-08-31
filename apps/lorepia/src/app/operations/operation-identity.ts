import type { GenerationSelectionInput } from '../../lib/ipc/contracts';

interface RetainedGenerationOperation {
    identity: string;
    nonce: string;
}

interface StagedGenerationAttemptRetry {
    identity: string | null;
    generationAttemptId: string;
}

export type GenerationOperationContext =
    | { kind: 'new'; authority: RetainedGenerationOperation }
    | {
          kind: 'resume';
          identity: string;
          generationAttemptId: string;
      };

export type GenerationOperationInputAuthority =
    { operation_nonce: string } | { generation_attempt_id: string };

export function generationSelectionOperationIdentity(
    selection: GenerationSelectionInput,
): readonly string[] {
    return selection.kind === 'target'
        ? [selection.kind, selection.target.model_route_id, selection.target.generation_preset_id]
        : [selection.kind, selection.provider_profile_id];
}

export function generationOperationIdentity(parts: readonly unknown[]): string {
    // Every caller supplies an explicit array with no object-valued members. JSON therefore
    // produces an unambiguous, order-stable identity without retaining user input in a map.
    return JSON.stringify(parts);
}

export class GenerationOperationIdentityAuthority {
    private retained: RetainedGenerationOperation | null = null;
    private stagedRetry: StagedGenerationAttemptRetry | null = null;

    beginNewOperation(): void {
        this.retained = null;
        this.stagedRetry = null;
    }

    stageAttemptRetry(generationAttemptId: string): boolean {
        if (
            generationAttemptId.length === 0 ||
            Array.from(generationAttemptId).length > 256 ||
            new TextEncoder().encode(generationAttemptId).byteLength > 512 ||
            /\p{Cc}/u.test(generationAttemptId)
        ) {
            return false;
        }
        this.stagedRetry = {
            identity: this.retained?.identity ?? null,
            generationAttemptId,
        };
        return true;
    }

    context(identity: string): GenerationOperationContext {
        const staged = this.stagedRetry;
        if (staged !== null) {
            staged.identity ??= identity;
            if (staged.identity === identity) {
                return {
                    kind: 'resume',
                    identity,
                    generationAttemptId: staged.generationAttemptId,
                };
            }
            this.stagedRetry = null;
        }
        return { kind: 'new', authority: this.authority(identity) };
    }

    complete(context: GenerationOperationContext): void {
        if (context.kind === 'new') {
            this.completeAuthority(context.authority);
            return;
        }
        const staged = this.stagedRetry;
        if (
            staged?.identity === context.identity &&
            staged.generationAttemptId === context.generationAttemptId
        ) {
            this.stagedRetry = null;
            if (this.retained?.identity === context.identity) {
                this.retained = null;
            }
        }
    }

    input(context: GenerationOperationContext): GenerationOperationInputAuthority {
        return context.kind === 'new'
            ? { operation_nonce: context.authority.nonce }
            : { generation_attempt_id: context.generationAttemptId };
    }

    private authority(identity: string): RetainedGenerationOperation {
        if (this.retained?.identity === identity) {
            return this.retained;
        }
        const authority = { identity, nonce: globalThis.crypto.randomUUID() };
        this.retained = authority;
        return authority;
    }

    private completeAuthority(authority: RetainedGenerationOperation): void {
        const retained = this.retained;
        if (retained?.identity === authority.identity && retained.nonce === authority.nonce) {
            this.retained = null;
        }
    }
}
