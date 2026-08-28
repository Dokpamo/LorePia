import {
    performPortableRegexOperation,
    type PortableRegexRequest,
} from './portable-regex-operation';

interface WorkerRequest {
    id: string;
    request: PortableRegexRequest;
}

const workerScope = self as unknown as {
    onmessage: ((event: MessageEvent<WorkerRequest>) => void) | null;
    postMessage: (message: unknown) => void;
};

workerScope.onmessage = (event) => {
    workerScope.postMessage({
        id: event.data.id,
        result: performPortableRegexOperation(event.data.request),
    });
};
