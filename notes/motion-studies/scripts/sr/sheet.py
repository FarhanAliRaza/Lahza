# usage: sheet.py framesdir out.png firstF lastF step cols width
import sys, os
from PIL import Image, ImageDraw, ImageFont
d,out,a,b,step,cols,w=sys.argv[1],sys.argv[2],int(sys.argv[3]),int(sys.argv[4]),int(sys.argv[5]),int(sys.argv[6]),int(sys.argv[7])
Fs=list(range(a,b+1,step)); h=w*9//16
font=ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf',max(10,w//14))
rows=(len(Fs)+cols-1)//cols
sh=Image.new('RGB',(cols*w,rows*h),'black'); dr=ImageDraw.Draw(sh)
for k,F in enumerate(Fs):
    im=Image.open(os.path.join(d,f"{F:04d}.png")).convert('RGB').resize((w,h))
    x,y=(k%cols)*w,(k//cols)*h; sh.paste(im,(x,y))
    t=f"F{F} {F*0.04:.2f}"; dr.rectangle([x,y,x+len(t)*w//22,y+w//12],fill='black'); dr.text((x+2,y),t,fill='yellow',font=font)
sh.save(out)
