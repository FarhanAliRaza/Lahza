import sys
from PIL import Image
R=sys.argv[1]
def load(n): return Image.open(f'{R}/{n:03d}.jpg').convert('RGB')
def L(p): return (p[0]*299+p[1]*587+p[2]*114)//1000
print("C) BlackLine black disc: dark run from (360,cy) up/left/right, threshold L>45")
for n in range(147,186):
    im=load(n); px=im.load()
    def run(dx,dy,x,y):
        r=0
        while 0<=x<720 and 0<=y<900:
            if L(px[x,y])>45 and not (300<y<620 and 150<x<570 and dy==0): return r
            x+=dx;y+=dy;r+=1
        return None
    up=None
    # vertical up from y=300 (above text) at x=360
    y=300
    while y>=0 and L(px[360,y])<=45: y-=1
    top=y if y>=0 else None
    # horizontal at row 300
    x=360
    while x<720 and L(px[x,300])<=45: x+=1
    right=x if x<720 else None
    x=360
    while x>=0 and L(px[x,300])<=45: x-=1
    left=x if x>=0 else None
    # text brightness (mean L of pixels in text box)
    s=c=0
    for yy in range(400,560,2):
        for xx in range(150,570,2):
            v=L(px[xx,yy]); 
            if v>60: s+=v;c+=1
    print(n,"top",top,"left",left,"right",right,"text_px",c,"text_meanL",round(s/c) if c else 0)
