import sys
from PIL import Image
from collections import Counter
for p in sys.argv[1:]:
    im=Image.open(p).convert('RGB')
    W,H=im.size
    px=im.load()
    def hx(c): return '#%02X%02X%02X'%c
    pts={'TL':(40,40),'TR':(W-40,40),'mid':(W//2,H//2),'BL':(40,H-40),'BR':(W-40,H-40),'top-c':(W//2,60)}
    s=' '.join(f"{k}={hx(px[x,y])}" for k,(x,y) in pts.items())
    # blue pixels median
    blues=[];whites=[]
    for y in range(0,H,2):
        for x in range(0,W,2):
            r,g,b=px[x,y]
            if b>120 and r<90 and g<90 and b-r>80: blues.append((r,g,b))
            if r>235 and g>235 and b>235: whites.append((r,g,b))
    def med(l):
        if not l: return '-'
        l2=sorted(l,key=sum); return hx(l2[len(l2)//2])+f"(n={len(l)})"
    q=im.quantize(6).convert('RGB')
    top=Counter(q.getdata()).most_common(6)
    print(p.split('/')[-1],s,'blue',med(blues),'white',med(whites),'q6',' '.join(f"{hx(c)}:{100*k//(W*H)}%" for c,k in top))
