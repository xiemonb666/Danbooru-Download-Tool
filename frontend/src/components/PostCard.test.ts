import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import type { DanbooruPost } from '../api'
import PostCard from './PostCard.vue'

const post: DanbooruPost = {
  id: 42,
  rating: 'q',
  score: 120,
  fav_count: 8,
  image_width: 1200,
  image_height: 1800,
  file_ext: 'jpg',
  file_size: 512000,
  is_video: false,
  is_ugoira: false,
  restricted: false,
  downloaded: false,
  tags: { general: ['landscape'], artist: [], copyright: [], character: [], meta: [] },
}

describe('PostCard', () => {
  it('uses the higher quality sample variant for the grid preview', () => {
    const view = render(PostCard, { props: { post, selected: false } })

    const image = view.getByRole('img')
    expect(image).toHaveAttribute('src', '/api/danbooru/posts/42/media/sample')
    expect(image).toHaveAttribute('width', '1200')
    expect(image).toHaveAttribute('height', '1800')
  })

  it('walks through independent preview variants when an upstream asset cannot be loaded', async () => {
    const view = render(PostCard, { props: { post, selected: false } })
    const image = view.getByRole('img')

    await fireEvent.error(image)

    expect(image).toHaveAttribute('src', '/api/danbooru/posts/42/media/preview')
    await fireEvent.error(image)
    expect(image).toHaveAttribute('src', '/api/danbooru/posts/42/media/large')
    await fireEvent.error(image)
    expect(image).toHaveAttribute('src', '/api/danbooru/posts/42/media/original')
    await fireEvent.error(image)
    expect(view.getByText('暂无可访问的预览')).toBeVisible()
  })

  it('lets the browser derive responsive height without cropping the image', () => {
    const view = render(PostCard, { props: { post, selected: false } })

    expect(view.getByRole('img')).toHaveStyle({
      width: '100%',
      height: 'auto',
      objectFit: 'contain',
    })
  })

  it('blurs questionable media until revealed while safe media stays visible', async () => {
    const view = render(PostCard, { props: { post, selected: false } })
    expect(view.getByRole('img')).toHaveClass('media-obscured')

    await fireEvent.click(view.getByRole('button', { name: '显示敏感内容' }))
    expect(view.getByRole('img')).not.toHaveClass('media-obscured')

    await view.rerender({ post: { ...post, rating: 's' }, selected: false })
    expect(view.getByRole('img')).not.toHaveClass('media-obscured')
  })

  it('shows questionable media directly when sensitive blur is disabled in settings', () => {
    const view = render(PostCard, {
      props: { post, selected: false, blurSensitive: false },
    })

    expect(view.getByRole('img')).not.toHaveClass('media-obscured')
    expect(view.queryByRole('button', { name: '显示敏感内容' })).not.toBeInTheDocument()
  })

  it('fails closed and labels an unknown upstream rating safely', () => {
    const unknownPost = { ...post, rating: 'unexpected-rating' } as unknown as DanbooruPost
    const view = render(PostCard, { props: { post: unknownPost, selected: false } })

    expect(view.getByRole('img')).toHaveClass('media-obscured')
    expect(view.getByText('Unknown')).toBeVisible()
    expect(view.getByRole('button', { name: '显示敏感内容' })).toBeVisible()
  })
})
