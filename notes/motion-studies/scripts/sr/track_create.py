import sys
from PIL import Image, ImageChops
d=sys.argv[1]
ref=Image.open(f"{d}/0059.png").convert('RGB')
def bbox_mag(im):
    px=im.load(); xs=[];ys=[]
    for y in range(200,520,2):
        for x in range(300,1000,2):
            r,g,b=px[x,y]
            if r>170 and g<110 and b>140: xs.append(x); ys.append(y)
    return (min(xs),min(ys),max(xs),max(ys)) if xs else None
for F in range(52,72):
    im=Image.open(f"{d}/{F:04d}.png").convert('RGB')
    bb=bbox_mag(im)
    line=f"F{F} t={F*0.04:.2f} button_bbox={bb} w={bb[2]-bb[0] if bb else 0} cx={(bb[0]+bb[2])/2 if bb else 0}"
    if F>=60:
        df=ImageChops.difference(im,ref).convert('L').load()
        pts=[(x,y) for y in range(300,720,2) for x in range(600,1100,2) if df[x,y]>60]
        if pts:
            tip=min(pts,key=lambda p:p[0]+p[1]); cx=sum(p[0] for p in pts)/len(pts); cy=sum(p[1] for p in pts)/len(pts)
            line+=f" cursor_tip={tip} n={len(pts)}"
    print(line)
