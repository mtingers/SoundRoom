/** JSON.parse alone accepts duplicate keys; native interchange must have one unambiguous value. */
export function parseStrictJSON(text: string): unknown {
    const stack: (Set<string> | undefined)[] = [];
    for (let i = 0; i < text.length; i++) {
        const c = text[i];
        if (c === '{' || c === '[') { if (stack.length >= 64) throw Error('JSON is too deeply nested.'); stack.push(c === '{' ? new Set() : undefined); }
        else if (c === '}' || c === ']') stack.pop();
        else if (c === '"') {
            const start = i; let escaped = false;
            for (i++; i < text.length; i++) { if (escaped) escaped = false; else if (text[i] === '\\') escaped = true; else if (text[i] === '"') break; }
            let next = i + 1; while (/[\t\r\n ]/.test(text[next] ?? 'x')) next++;
            const object = stack.at(-1);
            if (text[next] === ':' && object) { const key = JSON.parse(text.slice(start, i + 1)) as string; if (object.has(key)) throw Error('Duplicate JSON field.'); object.add(key); }
        }
    }
    return JSON.parse(text);
}
