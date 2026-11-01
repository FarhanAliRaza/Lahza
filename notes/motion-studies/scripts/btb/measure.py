import sys, math
from PIL import Image
R=sys.argv[1]
def load(n): return Image.open(f'{R}/{n:03d}.jpg').convert('RGB')
def L(p): return (p[0]*299+p[1]*587+p[2]*114)//1000
def disc_radius(n, thr, angles, start, cx=None, cy=None, dark=True):
    im=load(n); W,H=im.size; px=im.load()
    if cx is None:
        # centroid of bright text pixels near center region
        xs=ys=c=0
        for y in range(250,650,2):
            for x in range(120,600,2):
                if L(px[x,y])>200: xs+=x; ys+=y; c+=1
        cx,cy=(xs/c,ys/c) if c else (360,450)
    ds=[]
    for a in angles:
        r=start
        while True:
            x=int(cx+r*math.cos(math.radians(a))); y=int(cy+r*math.sin(math.radians(a)))
            if not(0<=x<W and 0<=y<H): ds.append(None); break
            v=L(px[x,y])
            if (v>thr) if dark else (v<thr): ds.append(r); break
            r+=1
    v=sorted(d for d in ds if d is not None)
    return (round(cx),round(cy)), (v[len(v)//2] if v else None), ds
print("A) shot1 black disc radius (rays down/up from text centroid)")
for n in range(5,30):
    c,r,ds=disc_radius(n,40,[60,75,90,105,120,240,255,270,285,300],25)
    print(n, c, r, ds)
