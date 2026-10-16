# List entries of a remote zip using HTTP range requests (central directory only).
import json, struct, sys, urllib.request
url, out = sys.argv[1], sys.argv[2]
def get(start, end):
    req = urllib.request.Request(url, headers={'Range': f'bytes={start}-{end}'})
    return urllib.request.urlopen(req, timeout=60).read()
size = int(urllib.request.urlopen(urllib.request.Request(url, method='HEAD'), timeout=30).headers['Content-Length'])
tail = get(max(0, size - 70000), size - 1)
i = tail.rfind(b'PK\x05\x06')
_, _, _, count, cd_size, cd_off, _ = struct.unpack('<IHHHHII', tail[i:i+20]) if False else (None,)*7
disk, cd_disk, n_disk, count, cd_size, cd_off, clen = struct.unpack('<HHHHIIH', tail[i+4:i+22])
if cd_off == 0xFFFFFFFF or count == 0xFFFF:  # zip64
    j = tail.rfind(b'PK\x06\x07'); z64off = struct.unpack('<Q', tail[j+8:j+16])[0]
    rec = get(z64off, z64off + 56)
    count, cd_size, cd_off = struct.unpack('<QQQ', rec[32:56])
cd = get(cd_off, cd_off + cd_size - 1)
entries, p = [], 0
while p < len(cd) and cd[p:p+4] == b'PK\x01\x02':
    (method, crc, csize, usize, nlen, xlen, klen, off) = (struct.unpack('<H', cd[p+10:p+12])[0], *struct.unpack('<III', cd[p+16:p+28]), *struct.unpack('<HHH', cd[p+28:p+34]), struct.unpack('<I', cd[p+42:p+46])[0])
    name = cd[p+46:p+46+nlen].decode('utf-8', 'replace'); extra = cd[p+46+nlen:p+46+nlen+xlen]
    q = 0
    while q + 4 <= len(extra):
        hid, hl = struct.unpack('<HH', extra[q:q+4])
        if hid == 1:
            vals = list(struct.unpack('<' + 'Q' * (hl // 8), extra[q+4:q+4+hl])); k = 0
            if usize == 0xFFFFFFFF: usize = vals[k]; k += 1
            if csize == 0xFFFFFFFF: csize = vals[k]; k += 1
            if off == 0xFFFFFFFF: off = vals[k]; k += 1
        q += 4 + hl
    entries.append({'name': name, 'method': method, 'csize': csize, 'usize': usize, 'offset': off})
    p += 46 + nlen + xlen + klen
json.dump({'url': url, 'size': size, 'entries': entries}, open(out, 'w'))
print(len(entries), 'entries; total uncompressed', sum(e['usize'] for e in entries))
