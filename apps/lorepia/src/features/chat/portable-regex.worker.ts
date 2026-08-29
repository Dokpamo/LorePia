import { performPortableRegexOperation } from './portable-regex-operation';
import {
    isPortableRegexWorkerRequest,
    type PortableRegexWorkerRequest,
} from './portable-regex-protocol';

const workerScope = self as unknown as {
    onmessage: ((event: MessageEvent<unknown>) => void) | null;
    postMessage: (message: unknown) => void;
};

workerScope.onmessage = (event) => {
    if (!isPortableRegexWorkerRequest(event.data)) return;
    const request: PortableRegexWorkerRequest = event.data;
    workerScope.postMessage({
        id: request.id,
        result: performPortableRegexOperation(request.request),
    });
};
