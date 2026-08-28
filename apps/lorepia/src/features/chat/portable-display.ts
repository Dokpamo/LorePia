import type { CharacterDisplayTransformDto } from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { runPortableRegex } from './portable-regex';

const MAX_PASSES = 256;
const MAX_OUTPUT_CHARS = 262_144;

export interface PortableDisplayContext {
    variables: Readonly<Record<string, string>>;
    chatIndex: number;
    lastMessageId: number;
    lastCharacterMessage?: string;
    characterName?: string;
    userName?: string;
}

export function hasPortableDisplayTransform(
    _source: string,
    transforms: readonly CharacterDisplayTransformDto[],
): boolean {
    return transforms.some((transform) => !isAssetTransform(transform));
}

export async function renderPortableDisplay(
    source: string,
    transforms: readonly CharacterDisplayTransformDto[],
    context: PortableDisplayContext,
): Promise<string> {
    return renderPortableMacros(
        await applyPortableTransforms(source, transforms, true),
        context,
        source,
    );
}

export async function applyPortableTransforms(
    source: string,
    transforms: readonly CharacterDisplayTransformDto[],
    skipAssetTransforms = false,
): Promise<string> {
    let output = source;
    for (const transform of transforms.slice(0, 128)) {
        if ((skipAssetTransforms && isAssetTransform(transform)) || transform.pattern.length > 4096)
            continue;
        const result = await runPortableRegex({
            operation: 'replace',
            source: output,
            pattern: transform.pattern,
            flags: safeFlags(transform.flags, true),
            replacement: transform.replacement,
        });
        if (!result.ok || typeof result.value !== 'string') continue;
        if (Array.from(result.value).length > MAX_OUTPUT_CHARS) return source;
        output = result.value;
    }
    return output;
}

function isAssetTransform(transform: CharacterDisplayTransformDto): boolean {
    return /<img/i.test(transform.pattern) || /<img/i.test(transform.replacement);
}

function safeFlags(flags: string, retainGlobal: boolean): string {
    const result = [...new Set(flags.split('').filter((flag) => 'dgimsuvy'.includes(flag)))]
        .filter((flag) => retainGlobal || (flag !== 'g' && flag !== 'y'))
        .join('');
    return result;
}

export function renderPortableMacros(
    value: string,
    context: PortableDisplayContext,
    originalSource: string,
): string {
    let output = value;
    for (let pass = 0; pass < MAX_PASSES; pass += 1) {
        const token = nextKnownToken(output);
        if (token === null) break;
        const replacement = evaluateToken(token.value, context, originalSource, pass);
        if (replacement === null) break;
        output = replaceBounded(output, token.start, token.end, replacement, originalSource);
    }
    for (let pass = 0; pass < MAX_PASSES; pass += 1) {
        const block = innermostBlock(output);
        if (block === null) break;
        const replacement = evaluateBlock(block.token, context)
            ? output.slice(block.bodyStart, block.elseStart ?? block.bodyEnd)
            : block.elseEnd === null
              ? ''
              : output.slice(block.elseEnd, block.bodyEnd);
        output = replaceBounded(output, block.start, block.end, replacement, originalSource);
        for (let expressionPass = 0; expressionPass < MAX_PASSES; expressionPass += 1) {
            const token = nextKnownToken(output);
            if (token === null) break;
            const replacementValue = evaluateToken(
                token.value,
                context,
                originalSource,
                expressionPass,
            );
            if (replacementValue === null) break;
            output = replaceBounded(
                output,
                token.start,
                token.end,
                replacementValue,
                originalSource,
            );
        }
    }
    return output;
}

function replaceBounded(
    source: string,
    start: number,
    end: number,
    replacement: string,
    fallback: string,
): string {
    const result = `${source.slice(0, start)}${replacement}${source.slice(end)}`;
    return Array.from(result).length <= MAX_OUTPUT_CHARS ? result : fallback;
}

interface TokenMatch {
    start: number;
    end: number;
    value: string;
}

function nextKnownToken(source: string): TokenMatch | null {
    const stack: number[] = [];
    for (let index = 0; index + 1 < source.length; index += 1) {
        const pair = source.slice(index, index + 2);
        if (pair === '{{') {
            stack.push(index);
            index += 1;
        } else if (pair === '}}') {
            const start = stack.pop();
            if (start !== undefined) {
                const value = source.slice(start + 2, index);
                if (!isBlockToken(value) && isKnownToken(value)) {
                    return { start, end: index + 2, value };
                }
            }
            index += 1;
        }
    }
    return null;
}

function isBlockToken(value: string): boolean {
    const token = value.trim();
    return token.startsWith('#') || token.startsWith('/') || token === 'else';
}

function isKnownToken(value: string): boolean {
    const token = value.trim();
    const name = (token.split('::', 1)[0] ?? '').toLocaleLowerCase();
    return (
        [
            'getvar',
            'getglobalvar',
            'equal',
            'notequal',
            'greater',
            'greater_equal',
            'greaterequal',
            'less',
            'less_equal',
            'lessequal',
            'and',
            'or',
            'not',
            'contains',
            'startswith',
            'roll',
            'pick',
            'lastmessageid',
            'chat_index',
            'lastcharmessage',
            'char',
            'user',
            'raw',
            '?',
        ].includes(name) || token.startsWith('? ')
    );
}

function evaluateToken(
    value: string,
    context: PortableDisplayContext,
    source: string,
    pass: number,
): string | null {
    const token = value.trim();
    const [rawName = '', ...args] = token.split('::').map((part) => part.trim());
    const name = rawName.toLocaleLowerCase();
    switch (name) {
        case 'getvar':
        case 'getglobalvar':
            return variableValue(context.variables, args[0] ?? '') ?? '0';
        case 'equal':
            return booleanText(compare(args[0], args[1]) === 0);
        case 'notequal':
            return booleanText(compare(args[0], args[1]) !== 0);
        case 'greater':
            return booleanText(compare(args[0], args[1]) > 0);
        case 'greater_equal':
        case 'greaterequal':
            return booleanText(compare(args[0], args[1]) >= 0);
        case 'less':
            return booleanText(compare(args[0], args[1]) < 0);
        case 'less_equal':
        case 'lessequal':
            return booleanText(compare(args[0], args[1]) <= 0);
        case 'and':
            return booleanText(args.every(truthy));
        case 'or':
            return booleanText(args.some(truthy));
        case 'not':
            return booleanText(!truthy(args[0] ?? ''));
        case 'contains':
            return booleanText((args[0] ?? '').includes(args[1] ?? ''));
        case 'startswith':
            return booleanText((args[0] ?? '').startsWith(args[1] ?? ''));
        case 'lastmessageid':
            return String(context.lastMessageId);
        case 'chat_index':
            return String(context.chatIndex);
        case 'lastcharmessage':
            return context.lastCharacterMessage ?? '';
        case 'char':
            return context.characterName ?? '';
        case 'user':
            return context.userName ?? t('chat.runtime.persona.default');
        case 'raw':
            return args[0] ?? '';
        case 'roll': {
            const sides = Math.max(1, Number.parseInt(args[0] ?? '1', 10) || 1);
            return String((stableNumber(`${source}\0${token}\0${String(pass)}`) % sides) + 1);
        }
        case 'pick':
            return args.length === 0
                ? ''
                : (args[stableNumber(`${source}\0${token}\0${String(pass)}`) % args.length] ?? '');
        case '?':
            return arithmetic(args.join('::'));
        default:
            return token.startsWith('? ') ? arithmetic(token.slice(1)) : null;
    }
}

function variableValue(
    variables: Readonly<Record<string, string>>,
    requested: string,
): string | undefined {
    const key = requested.trim();
    return variables[key] ?? variables[key.replace(/^toggle_/, '')];
}

function compare(left = '', right = ''): number {
    const leftNumber = Number(left.trim());
    const rightNumber = Number(right.trim());
    if (
        left.trim() !== '' &&
        right.trim() !== '' &&
        Number.isFinite(leftNumber) &&
        Number.isFinite(rightNumber)
    ) {
        return leftNumber === rightNumber ? 0 : leftNumber < rightNumber ? -1 : 1;
    }
    return left.trim().localeCompare(right.trim());
}

function truthy(value: string): boolean {
    return !['', '0', 'false', 'null', 'none'].includes(value.trim().toLocaleLowerCase());
}

function booleanText(value: boolean): string {
    return value ? '1' : '0';
}

function arithmetic(value: string): string {
    const expression = value.replaceAll(' ', '');
    const match = /^(-?\d+)([+-])(\d+)$/.exec(expression);
    if (match === null) return expression;
    const left = Number.parseInt(match[1] ?? '0', 10);
    const right = Number.parseInt(match[3] ?? '0', 10);
    return String(match[2] === '+' ? left + right : left - right);
}

interface BlockMatch {
    start: number;
    bodyStart: number;
    bodyEnd: number;
    elseStart: number | null;
    elseEnd: number | null;
    end: number;
    token: string;
}

function innermostBlock(source: string): BlockMatch | null {
    const tokens = [...source.matchAll(/\{\{([^{}]*)}}/g)];
    const stack: {
        start: number;
        end: number;
        token: string;
        elseStart: number | null;
        elseEnd: number | null;
    }[] = [];
    for (const match of tokens) {
        const token = (match[1] ?? '').trim();
        const start = match.index;
        const end = start + match[0].length;
        if (token.startsWith('#if') || token.startsWith('#when')) {
            stack.push({ start, end, token, elseStart: null, elseEnd: null });
        } else if (token === ':else') {
            const open = stack.at(-1);
            if (open !== undefined) {
                open.elseStart = start;
                open.elseEnd = end;
            }
        } else if (token === '/' || token === '/if' || token === '/when') {
            const open = stack.pop();
            if (open !== undefined) {
                return {
                    start: open.start,
                    bodyStart: open.end,
                    bodyEnd: start,
                    elseStart: open.elseStart,
                    elseEnd: open.elseEnd,
                    end,
                    token: open.token,
                };
            }
        }
    }
    return null;
}

function evaluateBlock(token: string, context: PortableDisplayContext): boolean {
    if (token.startsWith('#if')) return truthy(token.slice(3).replace(/^::/, '').trim());
    if (!token.startsWith('#when')) return false;
    const args = token
        .slice(5)
        .replace(/^::/, '')
        .split('::')
        .map((part) => part.trim());
    if (args.length === 0) return false;
    while (args.length > 1) {
        const condition = args.pop() ?? '';
        const operator = args.pop() ?? '';
        let result: string;
        switch (operator) {
            case 'not':
                result = booleanText(!whenTruthy(condition));
                break;
            case 'keep':
            case 'legacy':
                result = condition;
                break;
            case 'and':
                result = booleanText(whenTruthy(args.pop() ?? '') && whenTruthy(condition));
                break;
            case 'or':
                result = booleanText(whenTruthy(args.pop() ?? '') || whenTruthy(condition));
                break;
            case 'is':
            case '=':
            case '==':
            case '===':
                result = booleanText((args.pop() ?? '') === condition);
                break;
            case 'isnot':
            case '!=':
            case '!==':
                result = booleanText((args.pop() ?? '') !== condition);
                break;
            case 'var':
                result = booleanText(whenTruthy(variableValue(context.variables, condition) ?? ''));
                break;
            case 'toggle':
                result = booleanText(
                    whenTruthy(variableValue(context.variables, `toggle_${condition}`) ?? ''),
                );
                break;
            case 'vis':
                result = booleanText(
                    (variableValue(context.variables, args.pop() ?? '') ?? '') === condition,
                );
                break;
            case 'visnot':
                result = booleanText(
                    (variableValue(context.variables, args.pop() ?? '') ?? '') !== condition,
                );
                break;
            case 'tis':
                result = booleanText(
                    (variableValue(context.variables, `toggle_${args.pop() ?? ''}`) ?? '') ===
                        condition,
                );
                break;
            case 'tisnot':
                result = booleanText(
                    (variableValue(context.variables, `toggle_${args.pop() ?? ''}`) ?? '') !==
                        condition,
                );
                break;
            case '>':
            case '>=':
            case '<':
            case '<=': {
                const comparison = compare(args.pop(), condition);
                result = booleanText(
                    operator === '>'
                        ? comparison > 0
                        : operator === '>='
                          ? comparison >= 0
                          : operator === '<'
                            ? comparison < 0
                            : comparison <= 0,
                );
                break;
            }
            default:
                result = booleanText(whenTruthy(condition));
                break;
        }
        args.push(result);
    }
    return whenTruthy(args[0] ?? '');
}

function whenTruthy(value: string): boolean {
    return value.trim() === '1' || value.trim() === 'true';
}

function stableNumber(value: string): number {
    let hash = 2166136261;
    for (let index = 0; index < value.length; index += 1) {
        hash ^= value.charCodeAt(index);
        hash = Math.imul(hash, 16777619);
    }
    return hash >>> 0;
}
