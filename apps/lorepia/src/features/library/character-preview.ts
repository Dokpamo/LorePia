const TEMPLATE_MARKERS = ['{{', '}}'] as const;

export function characterDescriptionPreview(description: string): string {
    const boundary = TEMPLATE_MARKERS.reduce((earliest, marker) => {
        const index = description.indexOf(marker);
        return index >= 0 ? Math.min(earliest, index) : earliest;
    }, description.length);

    return description
        .slice(0, boundary)
        .replace(/^\s*#{1,6}\s*/u, '')
        .replace(/\s+/gu, ' ')
        .trim()
        .slice(0, 512);
}
