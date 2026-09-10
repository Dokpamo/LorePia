import type { ConversationListItem } from './navigation-types';

export function latestChatTimes(items: ConversationListItem[]): Map<string, string> {
    const latest = new Map<string, string>();
    for (const item of items) {
        const time = Date.parse(item.updatedAt);
        if (!Number.isFinite(time)) continue;
        const previous = latest.get(item.characterId);
        if (!previous || time > Date.parse(previous)) latest.set(item.characterId, item.updatedAt);
    }
    return latest;
}

export function chatActivityTime(value: string, locale: string, now = new Date()) {
    const date = new Date(value);
    if (!Number.isFinite(date.getTime())) return null;
    const full = new Intl.DateTimeFormat(locale, { dateStyle: 'long', timeStyle: 'short' }).format(
        date,
    );
    const sameDay = date.toDateString() === now.toDateString();
    const korean = new Intl.Locale(locale).language === 'ko';
    const short = new Intl.DateTimeFormat(
        locale,
        sameDay
            ? { hour: 'numeric', minute: '2-digit' }
            : {
                  year:
                      date.getFullYear() !== now.getFullYear()
                          ? korean
                              ? '2-digit'
                              : 'numeric'
                          : undefined,
                  month: korean ? 'long' : 'numeric',
                  day: 'numeric',
              },
    ).format(date);
    return { full, short };
}
