# usage: crop.py dir out a b step cols x0 y0 x1 y1 tw
import sys, os
from PIL import Image, ImageDraw, ImageFont
d,out=sys.argv[1],sys.argv[2]; a,b,step,cols=map(int,sys.argv[3:7]); x0,y0,x1,y1=map(int,sys.argv[7:11]); tw=int(sys.argv[11])
th=int(tw*(y1-y0)/(x1-x0)); Fs=list(range(a,b+1,step)); rows=(len(Fs)+cols-1)//cols
font=ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf',14)
sh=Image.new('RGB',(cols*tw,rows*th)); dr=ImageDraw.Draw(sh)
for k,F in enumerate(Fs):
    im=Image.open(os.path.join(d,f"{F:04d}.png")).convert('RGB').crop((x0,y0,x1,y1)).resize((tw,th))
    x,y=(k%cols)*tw,(k//cols)*th; sh.paste(im,(x,y)); dr.text((x+2,y+1),f"F{F}",fill='yellow',font=font)
sh.save(out)
