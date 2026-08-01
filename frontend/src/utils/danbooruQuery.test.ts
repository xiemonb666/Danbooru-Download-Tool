import { describe, expect, it } from 'vitest'
import { composeBatchDownloadQuery, composeDanbooruQuery, composeTagDownloadQuery } from './danbooruQuery'

describe('composeDanbooruQuery', () => {
  it('preserves native Danbooru syntax and appends only explicit quick filters', () => {
    expect(composeDanbooruQuery('(cat OR dog) -animated score:>=20', {
      rating: 'q', order: 'score', format: 'webm',
    })).toBe('(cat OR dog) -animated score:>=20 rating:q order:score filetype:webm')
  })

  it('bounds an otherwise global custom sort to a Danbooru-indexed recent window', () => {
    expect(composeDanbooruQuery('  ', { order: 'score' }))
      .toBe('age:<1month order:score')
  })

  it('adds a minimum megapixel constraint for the resolution filter', () => {
    expect(composeDanbooruQuery('cat_ears', { minimumMegapixels: '4' }))
      .toBe('cat_ears mpixels:>=4')
  })

  it('combines an include query with normalized excluded tags for batch download', () => {
    expect(composeTagDownloadQuery(
      '1girl cat_ears score:>=10',
      'animated, lowres  -watermark\ncomic',
    )).toBe('1girl cat_ears score:>=10 -animated -lowres -watermark -comic')
  })

  it('drops a standalone minus marker from batch exclusions', () => {
    expect(composeTagDownloadQuery('1girl', 'comic -')).toBe('1girl -comic')
  })

  it('keeps score priority out of the remote Danbooru query', () => {
    expect(composeBatchDownloadQuery({
      tags: 'carlotta_(wuthering_waves)',
      excludedTags: '1boy, comic',
      minimumScore: 12,
      prioritizeScore: true,
    })).toBe('carlotta_(wuthering_waves) -1boy -comic score:>=12')
  })

  it('keeps combined priorities out of the remote Danbooru query', () => {
    expect(composeBatchDownloadQuery({
      tags: '1girl',
      excludedTags: '',
      minimumScore: 0,
      prioritizeScore: true,
      prioritizeResolution: true,
    })).toBe('1girl score:>=0')
  })

  it('adds a shortest-edge resolution limit to batch downloads', () => {
    expect(composeBatchDownloadQuery({
      tags: '1girl',
      excludedTags: '',
      minimumScore: 10,
      minimumResolution: 2048,
      prioritizeScore: false,
    })).toBe('1girl score:>=10 width:>=2048 height:>=2048')
  })
})
