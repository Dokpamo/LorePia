export class SerializedMutation {
    private tail: Promise<void> = Promise.resolve();

    enqueue<T>(mutation: () => Promise<T>): Promise<T> {
        const pending = this.tail.then(mutation);
        this.tail = pending.then(
            () => undefined,
            () => undefined,
        );
        return pending;
    }
}
