/** View-local visibility only; the iframe bridge still owns its validated media. */
export function observePortableMediaVisibility(
    frame: HTMLIFrameElement,
    runtimeId: () => string,
): () => void {
    let disposed = false;
    let intersecting = typeof IntersectionObserver === 'undefined';
    const notify = () => {
        if (disposed) return;
        const id = runtimeId();
        if (!id) return;
        frame.contentWindow?.postMessage(
            {
                channel: 'lorepia-portable-renderer-v1',
                type: 'portable_visibility',
                runtimeId: id,
                visible: intersecting && document.visibilityState !== 'hidden',
            },
            '*',
        );
    };
    const observer =
        typeof IntersectionObserver === 'undefined'
            ? null
            : new IntersectionObserver(
                  ([entry]) => {
                      intersecting = entry?.isIntersecting ?? false;
                      notify();
                  },
                  { root: frame.closest('.ui-messages'), threshold: 0 },
              );
    observer?.observe(frame);
    frame.addEventListener('load', notify);
    document.addEventListener('visibilitychange', notify);
    notify();
    return () => {
        disposed = true;
        observer?.disconnect();
        frame.removeEventListener('load', notify);
        document.removeEventListener('visibilitychange', notify);
    };
}
