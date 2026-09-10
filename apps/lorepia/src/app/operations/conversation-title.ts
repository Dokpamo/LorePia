/** Suggest the next numbered default title without changing custom room names. */
export function nextConversationTitle(items: { title: string }[], base: string): string {
    const used = new Set(items.map((item) => item.title.trim()));
    let highest = used.has(base) ? 1 : 0;
    for (const title of used) {
        if (!title.startsWith(`${base} `)) continue;
        const number = Number(title.slice(base.length + 1));
        if (Number.isSafeInteger(number) && number >= 2 && title === `${base} ${String(number)}`)
            highest = Math.max(highest, number);
    }
    if (!highest) return base;
    let next = highest < Number.MAX_SAFE_INTEGER ? highest + 1 : 2;
    while (used.has(`${base} ${String(next)}`)) next += 1;
    return `${base} ${String(next)}`;
}
