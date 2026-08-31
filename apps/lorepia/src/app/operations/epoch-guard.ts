export class EpochGuard {
    private epoch = 0;

    current(): number {
        return this.epoch;
    }

    advance(): number {
        this.epoch += 1;
        return this.epoch;
    }

    isCurrent(epoch: number): boolean {
        return epoch === this.epoch;
    }
}
