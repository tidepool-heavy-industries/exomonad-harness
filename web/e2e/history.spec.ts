import { test, expect } from '@playwright/test'
import { snapshot } from './fixtures.mjs'
import { control, openFixture, view, observations } from './support'

const items=(page:any)=>page.getByRole('list',{name:'Retained request items'}).getByRole('listitem')
const inspector=(page:any)=>page.getByRole('region',{name:'Retained request history'})

test('history recovers from 503, bounds each page, renders text/raw and explicitly skips oversized items',async({page,request})=>{
  await openFixture(page,request,{historyUnavailable:true,historyOversized:true})
  await view(page,'Timeline')
  await page.getByRole('button',{name:'Inspect history',exact:true}).first().click()
  await expect(page.getByRole('alert')).toContainText('unavailable')
  await expect(page.getByRole('heading',{name:'Operator sign in'})).toHaveCount(0)
  await control(request,'config',{historyUnavailable:false})
  await page.getByRole('button',{name:'Retry history',exact:true}).click()
  await expect(items(page)).toHaveCount(50)
  await expect(items(page).first()).toContainText('preserved whitespace 🐚')
  await expect(items(page).first().getByLabel('Text item 0',{exact:true})).toHaveClass(/history-prose/)
  await expect(items(page).nth(2).getByRole('heading',{name:'Model tool call · read_file',exact:true})).toBeVisible()
  await expect(items(page).nth(2).getByLabel('Arguments item 2',{exact:true})).toHaveClass(/history-code/)
  const responseDisclosure=items(page).nth(3).getByText('JSON result · 2 fields · reason: tool_result',{exact:true})
  await expect(items(page).nth(3).getByRole('heading',{name:'Tool response',exact:true})).toBeVisible()
  await expect(responseDisclosure).toBeVisible()
  await expect(responseDisclosure.locator('..')).not.toHaveAttribute('open')
  await responseDisclosure.click()
  await expect(items(page).nth(3).getByLabel('Result item 3',{exact:true})).toContainText('ready_results')
  await items(page).first().getByText('Message details',{exact:true}).click()
  await page.getByRole('button',{name:'Show Raw item 0',exact:true}).click()
  await expect(page.getByLabel('Raw item 0',{exact:true})).toContainText('output_text')
  await expect(page.getByLabel('Raw item 1',{exact:true})).toContainText('future_unknown_item')
  await inspector(page).getByRole('button',{name:'Next page',exact:true}).click()
  await expect(page.getByRole('button',{name:'Skip this item',exact:true})).toBeVisible()
  await expect(items(page)).toHaveCount(0)
  await page.getByRole('button',{name:'Skip this item',exact:true}).click()
  await expect(items(page)).toHaveCount(50)
  await expect(items(page).first()).toContainText('Item 51')
  await inspector(page).getByRole('button',{name:'Previous page',exact:true}).click()
  await expect(page.getByRole('button',{name:'Skip this item',exact:true})).toBeVisible()
  await inspector(page).getByRole('button',{name:'Previous page',exact:true}).click()
  await expect(items(page)).toHaveCount(50)
  await expect(items(page).first()).toContainText('Item 0')
  const pageZeroReadsBeforeRefresh=(await observations(request)).historyReads.filter((r:any)=>r.offset===0).length
  await page.getByRole('button',{name:'Refresh history',exact:true}).click()
  await expect.poll(async()=> (await observations(request)).historyReads.filter((r:any)=>r.offset===0).length).toBe(pageZeroReadsBeforeRefresh+1)
})

test('obsolete history HTTP response cannot replace the current request',async({page,request})=>{
  await openFixture(page,request,{historyDelay:800})
  await view(page,'Timeline')
  await page.getByRole('button',{name:'Inspect history',exact:true}).nth(0).click()
  const oldId=(await observations(request)).historyReads[0]?.id
  await expect.poll(async()=> (await observations(request)).historyReads.length).toBe(1)
  await control(request,'config',{historyDelay:0})
  await page.getByRole('button',{name:'Close history',exact:true}).click()
  await page.getByRole('button',{name:'Inspect history',exact:true}).nth(1).click()
  await expect(items(page)).toHaveCount(50)
  const reads=(await observations(request)).historyReads
  const currentId=reads[1].id
  expect(currentId).not.toBe(reads[0].id)
  await expect(items(page).first()).toContainText(`Retained fixture message ${currentId}`)
  // Wait until the obsolete server read has completed, then assert the current
  // request's identity/content remains displayed; no cancellation assumption.
  await page.waitForTimeout(900)
  await expect(items(page).first()).toContainText(`Retained fixture message ${currentId}`)
  if(oldId) await expect(items(page).first()).not.toContainText(`Retained fixture message ${oldId}`)
})

test('only selected-request durable changes refresh the current bounded history page',async({page,request})=>{
  const data=snapshot()
  await openFixture(page,request,{snapshot:data})
  await view(page,'Timeline')
  await page.getByRole('button',{name:'Inspect history',exact:true}).first().click()
  await expect(items(page)).toHaveCount(50)
  const first=(await observations(request)).historyReads[0].id
  await control(request,'frame',{type:'event',event:{seq:11,event:{kind:'envelope.upsert',value:{id:'unrelated',recipient:'/operator',sender:'/elsewhere',type:'MESSAGE',payload:'Unrelated update'}}}})
  await page.waitForTimeout(100)
  expect((await observations(request)).historyReads).toHaveLength(1)
  await inspector(page).getByRole('button',{name:'Next page',exact:true}).click()
  await expect(items(page).first()).toContainText('Item 50')
  const record=data.requests.find(r=>r.id===first)!
  await control(request,'frame',{type:'event',event:{seq:12,event:{kind:'request.upsert',value:{...record,version:2}}}})
  await expect.poll(async()=> (await observations(request)).historyReads.length).toBe(3)
  expect((await observations(request)).historyReads[2].offset).toBe(50)
  await expect(items(page)).toHaveCount(50)
})

test('confirmed authorization loss from history returns to sign in',async({page,request})=>{
  await openFixture(page,request)
  await view(page,'Timeline')
  await control(request,'config',{authenticated:false,historyStatus:401})
  await page.getByRole('button',{name:'Inspect history',exact:true}).first().click()
  await expect(page.getByRole('heading',{name:'Operator sign in'})).toBeVisible()
})
