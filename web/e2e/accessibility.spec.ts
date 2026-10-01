import { test, expect } from '@playwright/test'
import { snapshot } from './fixtures.mjs'
import { control, openFixture, view, audit, zoomContext, baseURL } from './support'

for (const theme of ['light','dark'] as const) {
  test(`${theme} desktop and narrow production screens pass axe`, async ({page,request},testInfo)=>{
    await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'})
    await control(request,'reset',{authenticated:false})
    await page.goto('/')
    await expect(page.getByRole('heading',{name:'Operator sign in'})).toBeVisible()
    await audit(page,testInfo,`${theme}-desktop-sign-in`)
    await page.setViewportSize({width:390,height:844})
    await audit(page,testInfo,`${theme}-narrow-sign-in`)
    await page.setViewportSize({width:1280,height:800})
    const data=snapshot()
    data.actors[0].identity.actor='/root/'+ 'long-identity-'.repeat(18)
    await openFixture(page,request,{snapshot:data})
    for (const name of ['Tree','Timeline','Inbox','Host']) {
      await view(page,name)
      await audit(page,testInfo,`${theme}-desktop-${name.toLowerCase()}`)
      await page.setViewportSize({width:390,height:844})
      await audit(page,testInfo,`${theme}-narrow-${name.toLowerCase()}`)
      await page.setViewportSize({width:1280,height:800})
    }
    await view(page,'Timeline')
    await page.getByRole('button',{name:'Inspect history',exact:true}).first().click()
    await expect(page.getByRole('list',{name:'Retained request items'})).toBeVisible()
    await audit(page,testInfo,`${theme}-desktop-history`)
    await page.setViewportSize({width:390,height:844})
    await audit(page,testInfo,`${theme}-narrow-history`)
    const standalone=snapshot(); delete (standalone as any).hostRun; standalone.actors=[]
    await control(request,'reset',{snapshot:standalone})
    await page.goto(baseURL)
    await view(page,'Command')
    await audit(page,testInfo,`${theme}-narrow-command`)
    await page.setViewportSize({width:1280,height:800})
    await audit(page,testInfo,`${theme}-desktop-command`)
  })

  test(`${theme} actual browser zoom keeps all screens usable`, async ({request},testInfo)=>{
    await control(request,'reset',{authenticated:false})
    const {context,page}=await zoomContext(testInfo)
    try {
      await page.emulateMedia({colorScheme:theme,reducedMotion:'reduce'})
      await expect(page.getByRole('heading',{name:'Operator sign in'})).toBeVisible()
      await audit(page,testInfo,`${theme}-zoom-sign-in`)
      await control(request,'reset')
      await page.reload()
      await expect(page.getByRole('heading',{name:'Tree',exact:true})).toBeVisible()
      for (const name of ['Tree','Timeline','Inbox','Host']) {
        await view(page,name)
        await audit(page,testInfo,`${theme}-zoom-${name.toLowerCase()}`)
      }
      await view(page,'Timeline')
      await page.getByRole('button',{name:'Inspect history',exact:true}).first().click()
      await expect(page.getByRole('list',{name:'Retained request items'})).toBeVisible()
      await audit(page,testInfo,`${theme}-zoom-history`)
    } finally {await context.close()}
  })
}
