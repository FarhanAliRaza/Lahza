import sys
from PIL import Image
d=sys.argv[1]; a,b=int(sys.argv[2]),int(sys.argv[3]); cx=int(sys.argv[4]) if len(sys.argv)>4 else 640; cy=int(sys.argv[5]) if len(sys.argv)>5 else 360
dirn=sys.argv[6] if len(sys.argv)>6 else 'up'
def name(c):
    r,g,b=c
    P={'navy':(13,15,59),'white':(255,255,255),'yellow':(253,197,0),'cream':(251,234,211),'cyan':(147,204,217),'blue':(58,127,194),'coral':(232,85,88)}
    return min(P,key=lambda k:sum((x-y)**2 for x,y in zip(P[k],c)))
for n in range(a,b+1):
    p=Image.open(f"{d}/f{n:03d}.png").convert('RGB').load()
    segs=[];cur=None;start=0
    rng=range(0,cy) if dirn=='up' else range(0,640)
    for r in rng:
        c=p[cx,cy-r] if dirn=='up' else p[cx+r,cy] if cx+r<1280 else None
        if c is None: break
        k=name(c)
        if k!=cur:
            if cur: segs.append((cur,start,r))
            cur=k;start=r
    segs.append((cur,start,r))
    print(n,' '.join(f"{k}:{s}-{e}" for k,s,e in segs if e-s>1))
