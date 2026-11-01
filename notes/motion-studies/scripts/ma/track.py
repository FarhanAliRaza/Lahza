# track.py dir pred start end [x0 y0 x1 y1]  -> per-frame bbox/centroid/count of pixels matching predicate
import sys
from PIL import Image
d,pred,s,e=sys.argv[1],sys.argv[2],int(sys.argv[3]),int(sys.argv[4])
roi=list(map(int,sys.argv[5:9])) if len(sys.argv)>8 else None
P={
 'white':lambda r,g,b: r>200 and g>200 and b>200,
 'yellow':lambda r,g,b: r>170 and g>170 and b<110,
 'pink':lambda r,g,b: r>170 and g<110 and b>90,
 'blue':lambda r,g,b: b>170 and r<170 and b-r>60,
 'bright':lambda r,g,b: r+g+b>240,
}[pred]
for n in range(s,e+1):
    im=Image.open(f'{d}/{n:04d}.png').convert('RGB')
    if roi: im=im.crop(roi)
    W,H=im.size; px=im.load()
    xs=[];ys=[]
    for y in range(0,H,2):
        for x in range(0,W,2):
            if P(*px[x,y]): xs.append(x);ys.append(y)
    if not xs: print(n,'none');continue
    xs.sort();ys.sort();k=len(xs)
    # robust bbox: 1st-99th percentile
    lo=lambda a:a[int(k*0.01)]; hi=lambda a:a[int(k*0.99)-1 if k>1 else 0]
    print(n,'cnt',k,'bbox',lo(xs),lo(ys),hi(xs),hi(ys),'w',hi(xs)-lo(xs),'h',hi(ys)-lo(ys),'cx %.1f cy %.1f'%(sum(xs)/k,sum(ys)/k))
