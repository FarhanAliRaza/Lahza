import sys
from PIL import Image
R=sys.argv[1]
for n,label in [(20,"shot1 gradient"),(60,"line drawing"),(118,"is ring/halftone"),(175,"BlackLine"),(195,"sphere+orange line"),(320,"break free render"),(345,"closeup shapes"),(415,"rings render"),(450,"light tubes"),(240,"C4D viewport"),(530,"end card")]:
    im=Image.open(f'{R}/{n:03d}.jpg').convert('RGB').resize((180,225))
    q=im.quantize(colors=7,method=Image.Quantize.MEDIANCUT)
    pal=q.getpalette()[:21]; cnt=sorted(q.getcolors(),reverse=True)
    tot=sum(c for c,_ in cnt)
    print(f"{label:20s} n{n}: "+"  ".join(f"#{pal[i*3]:02x}{pal[i*3+1]:02x}{pal[i*3+2]:02x}:{100*c//tot}%" for c,i in cnt))
