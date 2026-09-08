import type { ProviderDiscoveryConnectionOptionsInput } from '../../lib/ipc/contracts';

export function discoveryConnectionOptions(): ProviderDiscoveryConnectionOptionsInput {
    return {
        values: [],
        api_base_path: null,
        timeout_seconds: 30,
        network_mode: 'public',
        local_network_approval: null,
    };
}
