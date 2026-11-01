import sys, math
from PIL import Image
R=sys.argv[1]
def load(n): return Image.open(f'{R}/{n:03d}.jpg').convert('RGB')
def L(p): return (p[0]*299+p[1]*587+p[2]*114)//1000
def row_profile_peaks(n,y,thr):
    im=load(n); px=im.load(); W,H=im.size
    xs=[x for x in range(W) if L(px[x,y])>thr]
    return xs
print("B) 'is'/'where' ring: horizontal row through center; outer edges of bright ring (L>thr)")
for n in list(range(110,131))+list(range(140,148)):
    im=load(n); px=im.load()
    # find ring center row: row with max count of bright pixels in central column band
    best=None
    for y in range(300,600,2):
        c=sum(1 for x in range(200,520,2) if L(px[x,y])>180)
        pass
    # ring center: use vertical scan at x=360 and horizontal at y found
    col=[y for y in range(0,900) if L(px[360,y])>150]
    # outer extents vertically
    top=min(col) if col else None; bot=max(col) if col else None
    cy=(top+bot)//2 if col else 450
    row=[x for x in range(0,720) if L(px[x,cy])>150]
    print(n, "vert",top,bot,"h",(min(row),max(row)) if row else None, "D_h", (max(row)-min(row)) if row else None, "D_v", (bot-top) if col else None)
