import { test, expect } from '@playwright/test'
import { snapshot } from './fixtures.mjs'
import { control, view, noOverflow } from './support'

for(const workers of [200,2000]) {
  test(`${workers} workers with many requests keep DOM rows bounded and unrelated tree DOM unchanged`,async({page,request},testInfo)=>{
    const data=snapshot(workers,20)
    await control(request,'reset',{snapshot:data})
    await page.addInitScript(()=>{
      const root=globalThis as any
      root.fixtureMetrics={started:performance.now(),longTasks:[],treeMutations:0}
      new PerformanceObserver(list=>{
        root.fixtureMetrics.longTasks.push(...list.getEntries().map(e=>({start:e.startTime,duration:e.duration})))
      }).observe({type:'longtask',buffered:true})
    })
    await page.goto('/')
    await expect(page.getByRole('heading',{name:'Tree',exact:true})).toBeVisible()
    const tree=page.getByRole('region',{name:'Worker hierarchy'})
    await expect(tree).toBeVisible()
    const rowCount=await tree.getByRole('link').count()
    expect(rowCount).toBeGreaterThan(0)
    expect(rowCount).toBeLessThanOrEqual(500)
    const initial=await page.evaluate(()=>({elapsed:performance.now()-(globalThis as any).fixtureMetrics.started,domNodes:document.querySelectorAll('*').length}))
    await tree.evaluate(element=>{
      new MutationObserver(records=>{(globalThis as any).fixtureMetrics.treeMutations+=records.length}).observe(element,{childList:true,subtree:true,characterData:true,attributes:true})
    })
    const updateStarted=await page.evaluate(()=>performance.now())
    await control(request,'frame',{type:'event',event:{seq:'11',event:{kind:'envelope.upsert',value:{id:'unrelated-update',sender:'/elsewhere',recipient:'/operator',type:'PROGRESS',payload:'Unrelated event for render observation',ordinal:'2001'}}}})
    await page.evaluate(()=>new Promise<void>(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve()))))
    const unrelated=await page.evaluate(start=>({elapsed:performance.now()-start,mutations:(globalThis as any).fixtureMetrics.treeMutations}),updateStarted)
    expect(unrelated.mutations).toBe(0)
    await control(request,'frame',{type:'event',event:{seq:'12',event:{kind:'actor.upsert',value:{...data.actors[1],lifecycle:'running'}}}})
    await expect(tree).toContainText('running')
    await view(page,'Timeline')
    const timelineRows=await page.getByRole('table',{name:'Conversation activity timeline'}).getByRole('row').count()
    expect(timelineRows).toBeGreaterThan(0)
    expect(timelineRows).toBeLessThanOrEqual(51)
    await view(page,'Inbox')
    const inboxRows=await page.getByRole('list',{name:'Inbox messages'}).getByRole('listitem').count()
    expect(inboxRows).toBeGreaterThan(0)
    expect(inboxRows).toBeLessThanOrEqual(50)
    await view(page,'Chat')
    const actorRows=await page.getByRole('complementary',{name:'Workers'}).getByRole('link').count()
    expect(actorRows).toBeGreaterThan(0)
    expect(actorRows).toBeLessThanOrEqual(100)
    await noOverflow(page)
    const client=await page.context().newCDPSession(page)
    const heap=await client.send('Runtime.getHeapUsage')
    const metrics=await page.evaluate(()=>({longTasks:(globalThis as any).fixtureMetrics.longTasks,finalDomNodes:document.querySelectorAll('*').length}))
    await testInfo.attach('structural-performance-observations',{body:JSON.stringify({workers,requests:data.requests.length,fixtureBytes:Buffer.byteLength(JSON.stringify(data)),initial,unrelated,rowCounts:{tree:rowCount,timeline:timelineRows,inbox:inboxRows,actors:actorRows},heap,metrics,limits:'Timings/heap retained as observations; gates assert bounded rows and unchanged unrelated tree DOM. DOM observations do not count internal React renders.'},null,2),contentType:'application/json'})
  })
}
