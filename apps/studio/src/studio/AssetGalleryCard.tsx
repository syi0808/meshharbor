import { CheckIcon, CubeIcon, ImageSquareIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { assetThumbnail } from './asset-thumbnails';
import { STUDIO_IMAGE_DRAG_TYPE } from './reference-drag';
import type { Asset } from './types';

const compactCount = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });

export function AssetGalleryCard({
  asset, selected, loadPreview, onSelect, onDragImage, showVersion,
}: {
  asset: Asset;
  selected: boolean;
  showVersion?: boolean;
  loadPreview: (asset: Asset, signal: AbortSignal) => Promise<Blob>;
  onSelect: () => void;
  onDragImage: (image: Asset | null) => void;
}) {
  const card = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  const [image, setImage] = useState<{ hash: string; url: string } | null>(null);
  const Icon = asset.kind === 'mesh' ? CubeIcon : ImageSquareIcon;
  const name = showVersion ? asset.name : asset.model?.name ?? asset.name;
  const triangles = Number(asset.metadata.triangles);

  useEffect(() => {
    if (visible || !card.current) return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(card.current);
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible) return;
    const controller = new AbortController();
    let url: string | undefined;
    setImage(null);
    void assetThumbnail(asset, loadPreview, controller.signal)
      .then(blob => {
        if (!blob || controller.signal.aborted) return;
        url = URL.createObjectURL(blob);
        setImage({ hash: asset.sha256, url });
      })
      .catch(() => undefined);
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [asset.id, asset.kind, asset.sha256, loadPreview, visible]);

  return (
    <Button type="button" variant="outline" ref={card} onClick={onSelect} aria-pressed={selected}
      draggable={asset.kind === 'image'} onDragStart={event => {
        if (asset.kind !== 'image') { event.preventDefault(); return; }
        event.dataTransfer.setData(STUDIO_IMAGE_DRAG_TYPE, asset.id);
        event.dataTransfer.effectAllowed = 'copy';
        onDragImage(asset);
      }} onDragEnd={() => onDragImage(null)}
      className={cn(
        'grid h-auto min-w-0 justify-normal gap-0 overflow-hidden rounded-lg bg-card p-0 text-left text-card-foreground transition-colors dark:bg-card',
        selected ? 'border-primary bg-accent hover:bg-accent dark:bg-accent' : 'hover:border-muted-foreground/60 hover:bg-muted',
      )}>
      <span className="relative grid aspect-square w-full min-w-0 place-items-center overflow-hidden bg-neutral-800">
        {image?.hash === asset.sha256
          ? <img src={image.url} alt="" data-asset-thumbnail="" draggable={false} decoding="async" className="absolute inset-0 size-full object-contain" />
          : <Icon className="size-7 text-muted-foreground/60" aria-hidden="true" />}
        <span aria-hidden="true" className="absolute bottom-1.5 left-1.5 rounded bg-black/70 px-1.5 py-0.5 text-[10px] leading-none font-medium text-white">
          {asset.kind === 'mesh' ? '3D' : 'IMG'}
        </span>
        {selected && <span className="absolute top-1.5 right-1.5 grid size-5 place-items-center rounded-full bg-primary text-primary-foreground">
          <CheckIcon className="size-3" weight="bold" aria-hidden="true" />
        </span>}
      </span>
      <span className="grid min-w-0 gap-0.5 px-2 py-1.5">
        <span dir="rtl" className="block min-w-0 truncate text-left text-xs font-medium" title={name}><bdi>{name}</bdi></span>
        <span className="block min-w-0 truncate text-[10px] tabular-nums text-muted-foreground">
          {asset.kind === 'image' ? 'Image' : Number.isFinite(triangles) ? `${compactCount.format(triangles)} tris` : 'Mesh'}
          {' · '}{showVersion && asset.model_version ? `Version ${asset.model_version}`
            : asset.model ? asset.model.generated ? 'Generated' : 'Imported' : asset.source_asset_id ? 'Result' : 'Original'}
        </span>
      </span>
    </Button>
  );
}
