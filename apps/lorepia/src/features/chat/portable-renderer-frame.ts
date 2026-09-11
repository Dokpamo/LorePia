import bridgeUrl from './portable-renderer-bridge.js?url&no-inline';
import type { PortableSurface } from './portable-renderer-policy';

const BASE_STYLE = `
    html, body { display: block; min-width: 0; max-width: 100%; margin: 0; color: inherit;
        font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        position: relative; contain: layout paint style; isolation: isolate; overflow: hidden; }
    .portable-message { max-width: 100%; white-space: pre-wrap; overflow: hidden;
        overflow-wrap: anywhere; }
    .portable-asset-frame { display: block; width: min(400px, 100%); margin: 12px auto; }
    .portable-asset-frame img { display: block; width: 100%; height: auto; border-radius: 10px; }
    .portable-audio { display: block; width: min(420px, 100%); margin: 10px auto; }
    .portable-asset-missing { display: block; padding: 8px 10px; border: 1px dashed currentColor;
        border-radius: 8px; opacity: .7; font-size: .85em; }
    details { white-space: normal; }
    button, input { font: inherit; }
    @media (max-width: 760px) {
        * { scrollbar-width: none; }
        *::-webkit-scrollbar { display: none; width: 0; height: 0; }
    }
`;

export function portableFrameDocument(
    content: string,
    importedStyle: string,
    runtimeId: string,
    activeSurface: PortableSurface,
): string {
    const nonce = globalThis.crypto.randomUUID().replaceAll('-', '');
    const scriptClose = '</scr' + 'ipt>';
    const scriptUrl = new URL(bridgeUrl, document.baseURI).href;
    const mediaSources =
        'lorepia-asset: http://lorepia-asset.localhost https://lorepia-asset.localhost';
    const csp = [
        "default-src 'none'",
        `script-src 'nonce-${nonce}'`,
        "style-src 'unsafe-inline'",
        `img-src ${mediaSources}`,
        `media-src ${mediaSources}`,
        "connect-src 'none'",
        "font-src 'none'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'none'",
        "frame-src 'none'",
        "frame-ancestors 'none'",
    ].join('; ');
    return [
        '<!doctype html><html><head><meta charset="utf-8">',
        `<meta http-equiv="Content-Security-Policy" content="${escapeHtml(csp)}">`,
        '<meta name="referrer" content="no-referrer">',
        `<style>${BASE_STYLE}${importedStyle}${activeSurface === 'room' ? 'html,body{height:100%;max-height:100%;overflow:hidden}.portable-message{height:100%;white-space:normal}' : ''}</style>`,
        '</head><body>',
        content,
        `<script nonce="${nonce}" src="${escapeHtml(scriptUrl)}" data-runtime-id="${escapeHtml(runtimeId)}">${scriptClose}`,
        '</body></html>',
    ].join('');
}

export function escapeHtml(value: string): string {
    return value
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;')
        .replaceAll('"', '&quot;')
        .replaceAll("'", '&#39;');
}
