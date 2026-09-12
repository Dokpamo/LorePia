export interface PortableDocumentResult {
    content: string;
    style: string;
}
interface Job {
    controller: AbortController;
    identity: readonly unknown[];
    build: (signal: AbortSignal) => Promise<PortableDocumentResult>;
    publish: (result: PortableDocumentResult, runtimeId: string, changed: boolean) => void;
}

/** One running build and one latest replacement per mounted frame. */
export class PortableDocumentQueue {
    private running: Job | null = null;
    private pending: Job | null = null;
    private published: { result: PortableDocumentResult; identity: readonly unknown[] } | null =
        null;
    private runtimeId = '';
    private accepting = false;

    get activeRuntimeId(): string {
        return this.runtimeId;
    }
    accepts(runtimeId: string): boolean {
        return this.accepting && runtimeId === this.runtimeId;
    }
    submit(identity: readonly unknown[], build: Job['build'], publish: Job['publish']): () => void {
        this.running?.controller.abort();
        this.pending?.controller.abort();
        const job: Job = { controller: new AbortController(), identity, build, publish };
        this.pending = job;
        this.accepting = false;
        this.drain();
        return () => {
            job.controller.abort();
            if (this.pending === job) this.pending = null;
            this.accepting = false;
        };
    }
    clear(): void {
        this.running?.controller.abort();
        this.pending?.controller.abort();
        this.pending = null;
        this.published = null;
        this.runtimeId = '';
        this.accepting = false;
    }
    private drain(): void {
        if (this.running || !this.pending) return;
        const job = this.pending;
        this.pending = null;
        this.running = job;
        void (async () => job.build(job.controller.signal))()
            .then((result) => {
                if (job.controller.signal.aborted) return;
                const previous = this.published;
                const changed =
                    previous?.result.content !== result.content ||
                    previous.result.style !== result.style ||
                    previous.identity.length !== job.identity.length ||
                    previous.identity.some(
                        (value, index) => !Object.is(value, job.identity[index]),
                    );
                if (changed) this.runtimeId = globalThis.crypto.randomUUID();
                job.publish(result, this.runtimeId, changed);
                this.published = { result, identity: job.identity };
                this.accepting = true;
            })
            .catch(() => {
                // Failed/cancelled builds never publish partially sanitized content.
            })
            .finally(() => {
                this.running = null;
                this.drain();
            });
    }
}
