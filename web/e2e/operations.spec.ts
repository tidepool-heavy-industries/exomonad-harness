import { test, expect } from '@playwright/test'
import { snapshot, target } from './fixtures.mjs'
import { control, openFixture, view, chooseWorker, observations } from './support'

const composer=(page:any)=>page.getByLabel('Message to selected actor',{exact:true})
const send=(page:any)=>page.getByRole('button',{name:'Send input',exact:true})

test('URL Back/reload preserves exact selection and per-tab draft; shortcuts respect typing and move heading focus',async({page,request})=>{
  await openFixture(page,request)
  await chooseWorker(page)
  const draft='Original unsent text\n  Unicode 🐚 and spaces  '
  await composer(page).fill(draft)
  const hostURL=page.url()
  expect(hostURL).not.toBe('http://127.0.0.1:4387/')
  await composer(page).press('End')
  await page.keyboard.type('g t')
  await expect(page.getByRole('heading',{name:'Chat',exact:true})).toBeVisible()
  await composer(page).fill(draft)
  await view(page,'Inbox')
  expect(page.url()).not.toBe(hostURL)
  await page.goBack()
  await expect(page.getByRole('heading',{name:'Chat',exact:true})).toBeVisible()
  await expect(composer(page)).toHaveValue(draft)
  expect(page.url()).toBe(hostURL)
  await page.reload()
  await expect(page.getByRole('heading',{name:'Chat',exact:true})).toBeVisible()
  await expect(composer(page)).toHaveValue(draft)
  expect(new URL(page.url()).searchParams.get('incarnation')).toBe(target.incarnation)
  await page.getByRole('heading',{name:'Chat',exact:true}).click()
  await page.keyboard.press('g'); await page.keyboard.press('l')
  await expect(page.getByRole('heading',{name:'Timeline',exact:true})).toBeFocused()
  const inspect=page.getByRole('button',{name:'Inspect history',exact:true}).first()
  await inspect.click()
  await expect(page.getByRole('heading',{name:/Request history/})).toBeFocused()
  await page.getByRole('button',{name:'Close history',exact:true}).click()
  await expect(inspect).toBeFocused()
  expect((await observations(request)).commands).toHaveLength(0)
})

for(const failure of ['storage','randomness'] as const) {
  test(`${failure} failure preserves the full draft and sends nothing`,async({page,request})=>{
    await openFixture(page,request)
    await chooseWorker(page)
    const draft='Do not lose this original\n  payload 🐚  '
    await composer(page).fill(draft)
    await page.evaluate(mode=>{
      if(mode==='storage'){
        const original=Storage.prototype.setItem
        Storage.prototype.setItem=function(key,value){
          if(key.startsWith('harness.embeddedCommands')) throw new Error('Fixture storage unavailable')
          return original.call(this,key,value)
        }
      }else Object.defineProperty(crypto,'getRandomValues',{value:()=>{throw new Error('Fixture secure randomness unavailable')}})
    },failure)
    await send(page).click()
    await expect(composer(page)).toHaveValue(draft)
    await expect(page.getByRole('alert').first()).toBeVisible()
    expect((await observations(request)).commands).toHaveLength(0)
  })
}

test('transport reconnect is observational, preserves draft and gates sends on a fresh snapshot; explicit retry keeps original operation',async({page,request})=>{
  const data=snapshot()
  await openFixture(page,request,{snapshot:data,receipt:'none'})
  await chooseWorker(page)
  const original='Original retained operation\n payload 🐚'
  await composer(page).fill(original)
  await send(page).click()
  await expect.poll(async()=> (await observations(request)).commands.length).toBe(1)
  const first=(await observations(request)).commands[0]
  const retained=page.locator(`[data-operation-id="${first.operation_id}"]`)
  await expect(retained).toContainText(target.actor)
  await expect(retained).toContainText(target.incarnation)
  await expect(retained.getByText('Original command JSON')).toBeVisible()
  await expect(retained.locator('details pre')).toHaveCount(0)
  await retained.getByText('Original command JSON').click()
  const commandJson = await retained.locator('details pre').textContent()
  expect(JSON.parse(commandJson!).text).toBe(original)
  const draft='Unsubmitted reconnect draft'
  await composer(page).fill(draft)
  await control(request,'config',{holdSnapshot:true})
  await control(request,'disconnect')
  await expect(send(page)).toBeDisabled()
  await expect(page.getByRole('heading',{name:'Operator sign in'})).toHaveCount(0)
  await expect(composer(page)).toHaveValue(draft)
  await expect.poll(async()=> (await observations(request)).connections).toBeGreaterThan(1)
  await expect(send(page)).toBeDisabled()
  expect((await observations(request)).commands).toHaveLength(1)
  await control(request,'config',{holdSnapshot:false})
  await control(request,'snapshot',data)
  await expect(send(page)).toBeEnabled()
  await expect(composer(page)).toHaveValue(draft)
  expect((await observations(request)).commands).toHaveLength(1)
  await page.locator(`[data-operation-id="${first.operation_id}"]`).getByRole('button',{name:'Retry same operation',exact:true}).click()
  await expect.poll(async()=> (await observations(request)).commands.length).toBe(2)
  expect((await observations(request)).commands[1]).toEqual(first)
  await expect(composer(page)).toHaveValue(draft)
})

test('a sequence gap coalesces snapshot requests and never submits the draft',async({page,request})=>{
  const data=snapshot()
  await openFixture(page,request,{snapshot:data})
  await chooseWorker(page)
  await composer(page).fill('Gap draft remains unsent')
  await control(request,'config',{holdSnapshot:true})
  const frame=(seq:number)=>({type:'event',event:{seq:String(seq),event:{kind:'envelope.upsert',value:{id:`gap-${seq}`,recipient:'/operator',sender:'/root',type:'MESSAGE',payload:'Gap fixture'}}}})
  await control(request,'frame',frame(12))
  await control(request,'frame',frame(13))
  await expect(send(page)).toBeDisabled()
  await expect.poll(async()=> (await observations(request)).snapshotRequests).toBe(1)
  await expect(composer(page)).toHaveValue('Gap draft remains unsent')
  expect((await observations(request)).commands).toHaveLength(0)
  await control(request,'snapshot',{...data,seq:'13'})
  await expect(send(page)).toBeEnabled()
  expect((await observations(request)).commands).toHaveLength(0)
})

test('actor replacement cannot retarget a selected incarnation or its draft',async({page,request})=>{
  const data=snapshot()
  await openFixture(page,request,{snapshot:data})
  await chooseWorker(page)
  await composer(page).fill('Draft for the original incarnation')
  const replaced=structuredClone(data)
  replaced.actors[1].identity.incarnation='replacement-incarnation'
  replaced.seq='11'
  await control(request,'snapshot',replaced)
  await expect(send(page)).toBeDisabled()
  expect(new URL(page.url()).searchParams.get('incarnation')).toBe(target.incarnation)
  await expect(composer(page)).toHaveValue('Draft for the original incarnation')
  expect((await observations(request)).commands).toHaveLength(0)
  await page.getByRole('complementary',{name:'Workers'}).getByRole('link',{name:target.actor,exact:true}).and(page.locator('a[href*="incarnation=replacement-incarnation"]')).click()
  await composer(page).fill('Explicit replacement draft')
  await send(page).click()
  await expect.poll(async()=> (await observations(request)).commands.length).toBe(1)
  expect((await observations(request)).commands[0].command.target).toEqual({...target,incarnation:'replacement-incarnation'})
})

for(const outcome of ['admitted','refused','unconfirmed','control_requested'] as const) {
  test(`live ${outcome} receipt reconciles the retained operation without reload`,async({page,request})=>{
    await openFixture(page,request,{receipt:outcome})
    await chooseWorker(page)
    if(outcome==='control_requested') await page.getByRole('button',{name:'Interrupt',exact:true}).click()
    else { await composer(page).fill('Explicit receipt fixture'); await send(page).click() }
    await expect.poll(async()=> (await observations(request)).commands.length).toBe(1)
    const command=(await observations(request)).commands[0]
    const row=page.locator(`[data-operation-id="${command.operation_id}"]`)
    await expect(row).toContainText(outcome==='admitted'?'input_admitted':outcome)
    if(outcome==='unconfirmed') await expect(page.getByRole('region',{name:'Retained browser operations'})).not.toContainText('Refused')
    if(outcome==='admitted') {
      await control(request,'frame',{type:'event',event:{seq:'12',event:{kind:'command.receipt',value:{commandId:command.operation_id,target:command.command.target,outcome:'unconfirmed',reason:'Weaker late fixture uncertainty'}}}})
      await expect(row).toContainText('input_admitted')
    }
  })
}

test('explicit sign out clears unsubmitted drafts while observational reload does not',async({page,request})=>{
  await openFixture(page,request)
  await chooseWorker(page)
  await composer(page).fill('Only deliberate signout clears this')
  await page.reload()
  await expect(composer(page)).toHaveValue('Only deliberate signout clears this')
  await page.getByRole('button',{name:'Sign out',exact:true}).click()
  await expect(page.getByRole('heading',{name:'Operator sign in'})).toBeVisible()
  await control(request,'config',{authenticated:true})
  await page.reload()
  await chooseWorker(page)
  await expect(composer(page)).toHaveValue('')
})

test('unsupported receipt outcome remains a protocol error and cannot settle or refuse an operation',async({page,request})=>{
  const data=snapshot()
  await openFixture(page,request,{snapshot:data,receipt:'none'})
  await chooseWorker(page)
  await composer(page).fill('Unknown-outcome operation')
  await send(page).click()
  await expect.poll(async()=> (await observations(request)).commands.length).toBe(1)
  const command=(await observations(request)).commands[0]
  await composer(page).fill('Preserve this later draft')
  await control(request,'config',{holdSnapshot:true})
  await control(request,'frame',{type:'event',event:{seq:'11',event:{kind:'command.receipt',value:{commandId:command.operation_id,target:command.command.target,outcome:'future_unrecognized_outcome',reason:'Unsupported fixture outcome'}}}})
  await expect(send(page)).toBeDisabled()
  await expect.poll(async()=> (await observations(request)).snapshotRequests).toBe(1)
  const row=page.locator(`[data-operation-id="${command.operation_id}"]`)
  await expect(row).not.toContainText('refused')
  await expect(row).not.toContainText('input_admitted')
  await expect(composer(page)).toHaveValue('Preserve this later draft')
  await expect(page.getByRole('heading',{name:'Operator sign in'})).toHaveCount(0)
  expect((await observations(request)).commands).toHaveLength(1)
  await control(request,'snapshot',{...data,seq:'11'})
  await expect(send(page)).toBeEnabled()
  expect((await observations(request)).commands).toHaveLength(1)
})
