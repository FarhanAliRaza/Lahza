import sys, math
from PIL import Image, ImageChops, ImageStat
R=sys.argv[1]
def load(n): return Image.open(f'{R}/{n:03d}.jpg').convert('RGB')
def L(p): return (p[0]*299+p[1]*587+p[2]*114)//1000
# duplicates + mean luma
prev=None; dups=[]; lum=[]
for n in range(0,545):
    im=load(n).convert('L').resize((180,225))
    lum.append(ImageStat.Stat(im).mean[0])
    if prev is not None:
        d=ImageStat.Stat(ImageChops.difference(im,prev)).mean[0]
        if d<0.35: dups.append(n)
    prev=im
print("DUP frames (diff<0.35 vs previous):",dups)
print("mean luma per frame:",[round(v) for v in lum])
print("E) wipe edge n394-406: rightmost x of viewport grey (r~g~b, 30<L<80) on row 150")
for n in range(394,407):
    px=load(n).load(); xs=[x for x in range(720) if 30<L(px[x,150])<80 and abs(px[x,150][0]-px[x,150][2])<8]
    print(n, max(xs) if xs else None)
print("F) tube ball (white, low sat, L>235) centroid n430-472")
for n in range(430,473):
    px=load(n).load(); sx=sy=c=0
    for y in range(0,900,2):
        for x in range(0,720,2):
            p=px[x,y]
            if L(p)>235 and max(p)-min(p)<25: sx+=x;sy+=y;c+=1
    print(n, (round(sx/c),round(sy/c),c) if c else None)
print("G) petal burst: mean radius of blue pixels from (360,450) n293-305")
for n in range(293,306):
    px=load(n).load(); rs=[]
    for y in range(0,900,2):
        for x in range(0,720,2):
            r,g,b=px[x,y]
            if b>150 and b>r+60: rs.append(math.hypot(x-360,y-450))
    rs.sort()
    print(n, len(rs), round(rs[len(rs)//2]) if rs else None, round(rs[int(len(rs)*.9)]) if rs else None)
print("H) stroke draw-on pixel count (purple stroke L>90, b>r) n41-110")
out=[]
for n in range(41,111):
    px=load(n).load(); c=0
    for y in range(0,900,2):
        for x in range(0,720,2):
            r,g,b=px[x,y]
            if b>140 and b>g+40 and L((r,g,b))>80: c+=1
    out.append((n,c))
print(out)
print("I) orange stroke draw (r>180,g>100,b<90) n185-218")
out=[]
for n in range(185,219):
    px=load(n).load(); c=0
    for y in range(0,900,2):
        for x in range(0,720,2):
            r,g,b=px[x,y]
            if r>170 and g>90 and b<100: c+=1
    out.append((n,c))
print(out)
