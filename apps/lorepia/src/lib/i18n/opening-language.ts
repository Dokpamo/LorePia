export function openingLanguage(language?: string | null): string {
    const names: Record<string, string> = { ko: '한국어', en: 'English', ja: '日本語', zh: '中文' };
    return language ? (names[language] ?? language) : '—';
}
