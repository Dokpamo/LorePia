import aria from './media/aria.png';
import kai from './media/kai.png';
import noa from './media/noa.png';
import sera from './media/sera.png';

const media = new Map([
    ['portrait-aria', aria],
    ['portrait-kai', kai],
    ['portrait-noa', noa],
    ['portrait-sera', sera],
    ['creator-aria', aria],
    ['creator-kai', kai],
    ['creator-noa', noa],
    ['creator-sera', sera],
]);

/** Only bundled, explicitly listed demo images can be presented here. */
export function previewImageSource(assetId: string): string | undefined {
    return media.get(assetId);
}
