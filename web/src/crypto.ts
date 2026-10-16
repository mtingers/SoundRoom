import {SHA256} from './sha256.js';
/** getRandomValues remains available on HTTP LAN pages; randomUUID does not. */
export function randomUUID(): string {
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6]! & 15) | 64;
    bytes[8] = (bytes[8]! & 63) | 128;
    const hex = Array.from(bytes, n => n.toString(16).padStart(2, '0')).join('');
    return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
}
function hash(bytes: Uint8Array): Uint8Array {
    return Uint8Array.from(new SHA256().update(bytes).digest().match(/../g)!, n => parseInt(n,16));
}
/** HMAC-SHA256 for HTTP trusted LAN pages where SubtleCrypto is unavailable. */
export function hmacSHA256(key: Uint8Array, message: Uint8Array): Uint8Array {
    const material = key.length > 64 ? hash(key) : key;
    const inner = new Uint8Array(64 + message.length), outer = new Uint8Array(96);
    for (let i=0;i<64;i++) { inner[i]=(material[i]??0)^0x36; outer[i]=(material[i]??0)^0x5c; }
    inner.set(message,64); outer.set(hash(inner),64);
    return hash(outer);
}
