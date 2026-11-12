#!/usr/bin/env python3
import json, math, struct, subprocess, tempfile, unittest, hashlib
from pathlib import Path
import numpy as np
from PIL import Image
import io, sys
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tools'))
from validate_glb import parse

def build_fixture(path,n=12,extra_sets=None,seam_x=None):
    # n x n quad grid: broad interior permits real edge collapses while preserving UV continuity.
    pos=[];uv=[];norm=[]
    for y in range(n+1):
        for x in range(n+1):
            pos.append((x/n-.5,y/n-.5,0.015*math.sin(x*.8)*math.sin(y*.7)))
            uv.append((x/n,y/n));norm.append((0,0,1))
    ix=[]
    right_seam={}
    if seam_x is not None:
        # Duplicate only the shared geometric seam row. The two chart wedges
        # retain distinct, consistently oriented UV coordinates.
        for y in range(n+1):
            old=y*(n+1)+seam_x
            right_seam[old]=len(pos)
            pos.append(pos[old]);uv.append((0.51,y/n));norm.append((0,0,1))
        for y in range(n+1):
            for x in range(n+1):
                i=y*(n+1)+x
                if x<=seam_x:
                    uv[i]=(0.49*x/seam_x,y/n)
                else:
                    uv[i]=(0.51+0.49*(x-seam_x)/(n-seam_x),y/n)
    for y in range(n):
        for x in range(n):
            a=y*(n+1)+x;b=a+1;c=a+n+1;d=c+1
            if seam_x is not None and x>=seam_x:
                a=right_seam.get(a,a);c=right_seam.get(c,c)
            ix.extend((a,b,d,a,d,c))
    for extra in extra_sets or []:
        base=len(pos)
        pos.extend(extra['pos']);uv.extend(extra['uv']);norm.extend(extra['norm'])
        if extra.get('absolute_faces'):ix.extend(i for face in extra['faces'] for i in face)
        else:ix.extend(base+i for face in extra['faces'] for i in face)
    # tiny embedded texture bytes to assert that image data survives the GLB rewrite.
    im=Image.new('RGBA',(4,4),(42,130,55,255));buf=io.BytesIO();im.save(buf,format='PNG');png=buf.getvalue()
    blob=bytearray();views=[];access=[]
    def add(data, count, typ, comp, target=None):
        while len(blob)%4:blob.append(0)
        off=len(blob);blob.extend(data)
        v={'buffer':0,'byteOffset':off,'byteLength':len(data)}
        if target:v['target']=target
        vi=len(views);views.append(v)
        a={'bufferView':vi,'componentType':comp,'count':count,'type':typ}
        if typ=='VEC3' and comp==5126 and data is not None:
            arr=np.frombuffer(data,dtype='<f4').reshape(-1,3);a['min']=arr.min(0).tolist();a['max']=arr.max(0).tolist()
        access.append(a);return len(access)-1
    pa=add(np.asarray(pos,dtype='<f4').tobytes(),len(pos),'VEC3',5126,34962)
    ua=add(np.asarray(uv,dtype='<f4').tobytes(),len(uv),'VEC2',5126,34962)
    na=add(np.asarray(norm,dtype='<f4').tobytes(),len(norm),'VEC3',5126,34962)
    ia=add(np.asarray(ix,dtype='<u4').tobytes(),len(ix),'SCALAR',5125,34963)
    while len(blob)%4:blob.append(0)
    image_off=len(blob);blob.extend(png);image_view=len(views);views.append({'buffer':0,'byteOffset':image_off,'byteLength':len(png)})
    doc={'asset':{'version':'2.0','generator':'test fixture'},'buffers':[{'byteLength':len(blob)}],'bufferViews':views,'accessors':access,
         'images':[{'bufferView':image_view,'mimeType':'image/png'}],'textures':[{'source':0}],
         'materials':[{'pbrMetallicRoughness':{'baseColorTexture':{'index':0},'baseColorFactor':[1,1,1,1]}}],
         'meshes':[{'primitives':[{'attributes':{'POSITION':pa,'TEXCOORD_0':ua,'NORMAL':na},'indices':ia,'material':0}]}],
         'nodes':[{'mesh':0}],'scenes':[{'nodes':[0]}],'scene':0}
    js=json.dumps(doc,separators=(',',':')).encode();js+=b' '*((-len(js))%4);binary=bytes(blob);binary+=b'\0'*((-len(binary))%4)
    glb=b'glTF'+struct.pack('<II',2,12+8+len(js)+8+len(binary))+struct.pack('<II',len(js),0x4e4f534a)+js+struct.pack('<II',len(binary),0x004e4942)+binary
    path.write_bytes(glb)

def write_custom_glb(path,pos,uv,norm,ix):
    im=Image.new('RGBA',(4,4),(42,130,55,255));buf=io.BytesIO();im.save(buf,format='PNG');png=buf.getvalue()
    blob=bytearray();views=[];access=[]
    def add(data,count,typ,comp,target=None):
        while len(blob)%4:blob.append(0)
        off=len(blob);blob.extend(data);v={'buffer':0,'byteOffset':off,'byteLength':len(data)}
        if target:v['target']=target
        vi=len(views);views.append(v);a={'bufferView':vi,'componentType':comp,'count':count,'type':typ}
        if typ=='VEC3' and comp==5126:
            arr=np.frombuffer(data,dtype='<f4').reshape(-1,3);a['min']=arr.min(0).tolist();a['max']=arr.max(0).tolist()
        access.append(a);return len(access)-1
    pa=add(np.asarray(pos,dtype='<f4').tobytes(),len(pos),'VEC3',5126,34962);ua=add(np.asarray(uv,dtype='<f4').tobytes(),len(uv),'VEC2',5126,34962);na=add(np.asarray(norm,dtype='<f4').tobytes(),len(norm),'VEC3',5126,34962);ia=add(np.asarray(ix,dtype='<u4').tobytes(),len(ix),'SCALAR',5125,34963)
    while len(blob)%4:blob.append(0)
    image_off=len(blob);blob.extend(png);image_view=len(views);views.append({'buffer':0,'byteOffset':image_off,'byteLength':len(png)})
    doc={'asset':{'version':'2.0','generator':'test fixture'},'buffers':[{'byteLength':len(blob)}],'bufferViews':views,'accessors':access,'images':[{'bufferView':image_view,'mimeType':'image/png'}],'textures':[{'source':0}],'materials':[{'pbrMetallicRoughness':{'baseColorTexture':{'index':0},'baseColorFactor':[1,1,1,1]}}],'meshes':[{'primitives':[{'attributes':{'POSITION':pa,'TEXCOORD_0':ua,'NORMAL':na},'indices':ia,'material':0}]}],'nodes':[{'mesh':0}],'scenes':[{'nodes':[0]}],'scene':0}
    js=json.dumps(doc,separators=(',',':')).encode();js+=b' '*((-len(js))%4);binary=bytes(blob);binary+=b'\0'*((-len(binary))%4)
    glb=b'glTF'+struct.pack('<II',2,12+8+len(js)+8+len(binary))+struct.pack('<II',len(js),0x4e4f534a)+js+struct.pack('<II',len(binary),0x004e4942)+binary;path.write_bytes(glb)

def geometric_topology(data):
    p=data['doc']['meshes'][0]['primitives'][0];pos=data['decoded'][p['attributes']['POSITION']];faces=data['decoded'][p['indices']].reshape(-1,3)
    def vertex(i):return tuple(float(x) for x in pos[i])
    edges={};gfaces={}
    for tri in faces:
        xyz=[vertex(int(i)) for i in tri];key=tuple(sorted(xyz));gfaces[key]=gfaces.get(key,0)+1
        for k in range(3):
            edge=tuple(sorted((xyz[k],xyz[(k+1)%3])));edges[edge]=edges.get(edge,0)+1
    return {'boundary_edges':sum(v==1 for v in edges.values()),'nonmanifold_edges':sum(v>2 for v in edges.values()),'duplicate_faces':sum(v-1 for v in gfaces.values() if v>1)}

def split_uv_group_count(data):
    p=data['doc']['meshes'][0]['primitives'][0];pos=data['decoded'][p['attributes']['POSITION']];uv=data['decoded'][p['attributes']['TEXCOORD_0']];groups={}
    for xyz,st in zip(pos,uv):groups.setdefault(tuple(float(x) for x in xyz),set()).add(tuple(float(x) for x in st))
    return sum(len(values)>1 for values in groups.values())

class PipelineTests(unittest.TestCase):
    def test_simplifies_and_preserves_embedded_material(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'grid.glb';out=Path(td)/'grid-out.glb';report=Path(td)/'audit.tsv';build_fixture(src)
            subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'80',str(report)],check=True,capture_output=True,text=True)
            a=parse(src);b=parse(out)
            self.assertGreater(a['triangles'][0], b['triangles'][0]);self.assertLessEqual(b['triangles'][0],80);self.assertGreater(b['triangles'][0],0)
            topo_src=geometric_topology(a);topo_out=geometric_topology(b)
            self.assertLessEqual(topo_out['nonmanifold_edges'],topo_src['nonmanifold_edges'])
            self.assertLessEqual(topo_out['duplicate_faces'],topo_src['duplicate_faces'])
            self.assertEqual(a['images'],b['images']);self.assertEqual(a['doc']['materials'],b['doc']['materials'])
            p=b['doc']['meshes'][0]['primitives'][0];self.assertEqual(set(p['attributes']),{'POSITION','NORMAL','TEXCOORD_0'})
            self.assertEqual(b['decoded'][p['attributes']['POSITION']].shape[0],b['decoded'][p['attributes']['TEXCOORD_0']].shape[0])
            self.assertIn('component_id',report.read_text())
    def test_rejects_non_glb(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'bad.glb';src.write_bytes(b'not a glb')
            r=subprocess.run([str(ROOT/'glb_refine'),str(src),str(Path(td)/'o.glb')],capture_output=True,text=True)
            self.assertNotEqual(r.returncode,0)
    def test_tiny_detached_triangle_and_small_leaf_are_retained(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'grid-with-islands.glb';out=Path(td)/'refined.glb';report=Path(td)/'audit.tsv'
            tiny={'pos':[(5,5,5),(5.001,5,5),(5,5.001,5)],'uv':[(0,0),(1,0),(0,1)],'norm':[(0,0,1)]*3,'faces':[(0,1,2)]}
            leaf={'pos':[(2,2,2),(2.05,2,2),(2.05,2.05,2),(2,2.05,2)],'uv':[(0,0),(1,0),(1,1),(0,1)],'norm':[(0,0,1)]*4,'faces':[(0,1,2),(0,2,3)]}
            build_fixture(src,extra_sets=[leaf,tiny]);subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'80',str(report)],check=True,capture_output=True,text=True)
            data=parse(out);p=data['doc']['meshes'][0]['primitives'][0];positions=data['decoded'][p['attributes']['POSITION']]
            self.assertTrue(np.any(np.all(np.isclose(positions,[5,5,5],atol=1e-7),axis=1)))
            self.assertTrue(np.any(np.all(np.isclose(positions,[2.05,2.05,2],atol=1e-7),axis=1)))
            text=report.read_text();self.assertIn('review-tiny-detached-candidate; retained-and-pinned',text);self.assertIn('keep-small-component-pinned',text)
            self.assertIn('retain-and-pin',text)
    def test_attached_needle_is_flagged_and_pinned_not_deleted(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'grid-with-spike.glb';out=Path(td)/'refined.glb';report=Path(td)/'audit.tsv'
            # A tiny, lifted sliver shares its full base edge with the surface and disagrees with its smooth normals.
            tip=(-.25,-.5001,.0001)
            spike={'pos':[tip],'uv':[(.5,0)],'norm':[(0,0,1)],'faces':[(0,1,9)],'absolute_faces':True}
            build_fixture(src,n=2,extra_sets=[spike]);subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'5',str(report)],check=True,capture_output=True,text=True)
            data=parse(out);p=data['doc']['meshes'][0]['primitives'][0];positions=data['decoded'][p['attributes']['POSITION']]
            self.assertTrue(np.any(np.all(np.isclose(positions,tip,atol=1e-7),axis=1)))
            text=report.read_text();self.assertIn('Attached needle-like polygon flags',text);self.assertIn('retain-and-pin',text);self.assertIn('\n8\t',text)
    def test_synchronized_uv_seam_collapse_keeps_two_uv_charts(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'seamed-grid.glb';out=Path(td)/'seamed-grid-out.glb';report=Path(td)/'audit.tsv'
            build_fixture(src,n=12,seam_x=6)
            subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'80',str(report)],check=True,capture_output=True,text=True)
            text=report.read_text();self.assertIn('seam_sync_accepted=',text)
            accepted=int(text.split('seam_sync_accepted=')[1].split()[0])
            removed=int(text.split('seam_sync_removed_faces=')[1].split()[0])
            self.assertGreater(accepted,0,'paired UV seam edges were not collapsed together')
            self.assertEqual(removed,2*accepted)
            src_data=parse(src);data=parse(out);p=data['doc']['meshes'][0]['primitives'][0]
            pos=data['decoded'][p['attributes']['POSITION']];uv=data['decoded'][p['attributes']['TEXCOORD_0']]
            # At least one surviving exact-position seam wedge still has two distinct UV values.
            groups={}
            for xyz,st in zip(pos,uv):
                groups.setdefault(tuple(np.round(xyz,6)),set()).add(tuple(np.round(st,5)))
            self.assertTrue(any(len(values)>1 for values in groups.values()),'UV split was merged across geometric seam')
            self.assertEqual(split_uv_group_count(src_data),13)
            self.assertEqual(split_uv_group_count(data),7)
            src_top=geometric_topology(src_data);out_top=geometric_topology(data)
            self.assertEqual(src_top['boundary_edges'],out_top['boundary_edges'])
            self.assertEqual(out_top['nonmanifold_edges'],0)
            self.assertEqual(out_top['duplicate_faces'],0)
            self.assertLessEqual(data['triangles'][0],80)
            self.assertIn('rejected_uv_orientation=',text)
    def test_ambiguous_three_wedge_seam_is_retained(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'ambiguous-seam.glb';out=Path(td)/'ambiguous-seam-out.glb';report=Path(td)/'audit.tsv'
            n=12;seam_x=6
            third=[(0.0,y/n-.5,0.015*math.sin(seam_x*.8)*math.sin(y*.7)) for y in range(n+1)]
            third_uv=[(0.25,y/n) for y in range(n+1)]
            extra={'pos':third,'uv':third_uv,'norm':[(0,0,1)]*(n+1),'faces':[]}
            build_fixture(src,n=n,seam_x=seam_x,extra_sets=[extra])
            subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'80',str(report)],check=True,capture_output=True,text=True)
            text=report.read_text();accepted=int(text.split('seam_sync_accepted=')[1].split()[0])
            self.assertEqual(accepted,0,'ambiguous three-wedge seam should remain locked')
            data=parse(out);self.assertGreater(data['triangles'][0],80)
    def test_coincident_duplicate_shells_are_not_welded(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'coincident-shells.glb';out=Path(td)/'coincident-shells-out.glb';report=Path(td)/'audit.tsv'
            n=8;pos=[];uv=[];norm=[];faces=[]
            for y in range(n+1):
                for x in range(n+1):
                    pos.append((x/n-.5,y/n-.5,0.015*math.sin(x*.8)*math.sin(y*.7)))
                    uv.append((x/n,y/n));norm.append((0,0,1))
            for y in range(n):
                for x in range(n):
                    a=y*(n+1)+x;b=a+1;c=a+n+1;d=c+1;faces.extend(((a,b,d),(a,d,c)))
            extra={'pos':pos,'uv':uv,'norm':norm,'faces':faces,'absolute_faces':True}
            build_fixture(src,n=n,extra_sets=[extra])
            subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'80',str(report)],check=True,capture_output=True,text=True)
            text=report.read_text();accepted=int(text.split('seam_sync_accepted=')[1].split()[0])
            self.assertEqual(accepted,0,'coincident duplicate surfaces must not be paired as a UV seam')
            data=parse(out);self.assertEqual(data['triangles'][0],4*n*n)
    def test_nonmanifold_one_ring_vetoes_seam_collapse(self):
        with tempfile.TemporaryDirectory() as td:
            src=Path(td)/'tetra-seam-nonmanifold.glb';out=Path(td)/'tetra-seam-nonmanifold-out.glb';report=Path(td)/'audit.tsv'
            # Two tetrahedral shells share X-Y; A-B is a two-wedge UV seam.
            pos=[];uv=[];norm=[];ix=[]
            A=np.array((0,.1,.001));B=np.array((0,.1,-.001));X=np.array((-.1,0,0));Y=np.array((.1,0,0));Z=np.array((0,-.1,.1));W=np.array((0,-.1,-.1))
            def add(p,st,n):pos.append(tuple(p));uv.append(tuple(st));norm.append(tuple(n));return len(pos)-1
            def tri(a,b,c):ix.extend((a,b,c))
            fn=np.cross(B-A,X-A);fn=fn/np.linalg.norm(fn)
            a=add(A,(0,0),(0,0,1));b=add(B,(.01,0),(0,0,-1));x=add(X,(.5,1),fn);y0=add(Y,(-1,1),(0,0,1));y1=add(Y,(2,1),(0,0,-1))
            tri(a,b,x);tri(a,x,y0);tri(b,y1,x)
            island=0
            def separate(p0,p1,p2):
                nonlocal island
                base=4+island*3;island+=1;nn=np.cross(p1-p0,p2-p0);nn=nn/np.linalg.norm(nn)
                i0=add(p0,(base,0),nn);i1=add(p1,(base+1,0),nn);i2=add(p2,(base,1),nn);tri(i0,i1,i2)
            separate(B,A,Y);separate(X,Y,Z);separate(Y,X,W);separate(X,Z,W);separate(Y,W,Z)
            write_custom_glb(src,pos,uv,norm,ix)
            subprocess.run([str(ROOT/'glb_refine'),str(src),str(out),'1',str(report),'0.004','1.25','0.8660254037844386'],check=True,capture_output=True,text=True)
            text=report.read_text();accepted=int(text.split('seam_sync_accepted=')[1].split()[0]);nonmanifold=int(text.split('seam_sync_rejected_nonmanifold=')[1].split()[0])
            self.assertEqual(accepted,0)
            self.assertGreater(nonmanifold,0,text)
            data=parse(out);self.assertEqual(data['triangles'][0],8)
            self.assertEqual(geometric_topology(data)['duplicate_faces'],0)

if __name__=='__main__':unittest.main(verbosity=2)
