import type { LorepiaClient } from '../../lib/ipc/contracts';
import { loadAssetDelivery } from '../assets/asset-delivery-loader';

/** Per-build deduplication only; approvals are not cached across builds. */
export async function resolvePortableAssets(
    references: readonly string[],
    select: (reference: string) => { asset_id: string } | null,
    client: Pick<LorepiaClient, 'resolveAssetDelivery'>,
    signal: AbortSignal,
    url: (digest: string) => string | null,
): Promise<Map<string, string | null>> {
    const deliveries = new Map<string, Promise<string | null>>();
    const resolved = new Map<string, string | null>();
    for (let offset = 0; offset < references.length; offset += 8) {
        signal.throwIfAborted();
        await Promise.all(
            references.slice(offset, offset + 8).map(async (reference) => {
                const asset = select(reference);
                if (!asset) {
                    resolved.set(reference, null);
                    return;
                }
                let delivery = deliveries.get(asset.asset_id);
                if (!delivery) {
                    const id = asset.asset_id;
                    delivery = loadAssetDelivery(
                        client,
                        { kind: 'asset_id', asset_id: id },
                        signal,
                        undefined,
                        () => 0,
                        { maxAttempts: 8 },
                    )
                        .then((value) => (value.asset_id === id ? url(value.sha256) : null))
                        .catch((error: unknown) => {
                            signal.throwIfAborted();
                            void error;
                            return null;
                        });
                    deliveries.set(id, delivery);
                }
                resolved.set(reference, await delivery);
            }),
        );
    }
    signal.throwIfAborted();
    return resolved;
}
