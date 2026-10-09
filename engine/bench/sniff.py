# Records TCP segments on an interface (an L3 TUN): time, direction, seq, ack, len, options.
# sniff.py <iface> <port> <out.tsv>
import socket, struct, sys, time
iface, port, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.ntohs(0x0003)); s.bind((iface, 0))
f = open(out, "w", buffering=1); t0 = time.time()
while True:
    pkt = s.recv(65535)
    if len(pkt) < 40 or pkt[0] >> 4 != 4 or pkt[9] != 6: continue
    ihl = (pkt[0] & 15) * 4; tot = struct.unpack(">H", pkt[2:4])[0]; t = pkt[ihl:]
    sp, dp, seq, ack = struct.unpack(">HHII", t[:12]); off = (t[12] >> 4) * 4; flags = t[13]
    if port not in (sp, dp): continue
    win = struct.unpack(">H", t[14:16])[0]
    opts, o = [], t[20:off]
    j = 0
    while j < len(o):
        k = o[j]
        if k == 0: break
        if k == 1: j += 1; continue
        l = o[j + 1] if j + 1 < len(o) else 2
        if k == 5: opts.append("sack%d" % ((l - 2) // 8))
        elif k == 8: opts.append("ts")
        elif k == 2: opts.append("mss%d" % struct.unpack(">H", o[j+2:j+4])[0])
        elif k == 3: opts.append("ws%d" % o[j+2])
        elif k == 4: opts.append("sackok")
        j += max(l, 2)
    plen = tot - ihl - off
    d = "C>S" if dp == port else "S>C"
    f.write(f"{time.time()-t0:.4f}\t{d}\t{seq}\t{ack}\t{plen}\t{flags:02x}\t{win}\t{','.join(opts)}\n")
