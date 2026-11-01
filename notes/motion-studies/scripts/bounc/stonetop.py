import sys
W,H=640,360
f=sys.stdin.buffer;n=0
cols=(200,250,450,520)
out=[]
while True:
    b=f.read(W*H*3)
    if len(b)<W*H*3: break
    tops=[]
    for c in cols:
        top=None
        for y in range(H-1,100,-1):
            i=(y*W+c)*3; g=b[i+1]
            # stone darker shadow side or brighter? find first from bottom where pixel equals bg-ish -> scanning up until background
        # simpler: scan downward from y=215 (below title) to find first strong luminance change vs bg at y=215
        i0=(215*W+c)*3; bg=b[i0+1]
        for y in range(216,H):
            i=(y*W+c)*3
            if abs(b[i+1]-bg)>18: top=y;break
        tops.append(top)
    print(n,*tops); n+=1
    if n>200: break
