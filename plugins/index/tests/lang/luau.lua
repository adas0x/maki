local th = require("maki.test_helpers")
local helpers = require("tests.helpers")
local case = th.case
local idx = helpers.idx
local has = helpers.has
local lacks = helpers.lacks

case("luau_all_sections", function()
  local src = [==[
local Config = require("game.config")
local Deep = require("game.config.deep")
local Types = require("./types")
require("init")

local MAX_PLAYERS = 100
local MAX_RETRIES: number = 3
local state = {}

export type Options = {
  verbose: boolean,
  retries: number?,
}

type Alias = string

--- Runs the loop.
local function helper(x: number): (boolean, string?)
  return tostring(x)
end

function M.run<T>(opts: Options?): Options
  state.count += 1
  return opts
end

function M:handle(self, evt)
end

Module.Init = function(ctx: Context)
end

M.VERSION = "1.0"
Config.DEBUG = true
local debug_flag = false

return M
]==]
  local out = idx(src, "luau")
  has(out, {
    "imports:",
    "game.{config, config.deep}",
    "./types",
    "init",
    "consts:",
    "MAX_PLAYERS = 100",
    "MAX_RETRIES = 3",
    'M.VERSION = "1.0"',
    "Config.DEBUG = true",
    "types:",
    "export type Options = { verbose: boolean, retries: number?, }",
    "type Alias = string",
    "fns:",
    "helper(x: number): (boolean, string?) [17-20]",
    "M.run<T>(opts: Options?): Options",
    "M:handle(self, evt)",
    "Module.Init(ctx: Context)",
  })
  lacks(out, { "state = {}", "debug_flag" })
end)

case("luau_type_truncation", function()
  local src = [==[
type ReallyLong = {
  one: string,
  two: string,
  three: string,
  four: string,
  five: string,
}
]==]
  local out = idx(src, "luau")
  has(out, { "types:", "type ReallyLong = { one: string, two: string,", "[truncated]" })
end)

case("luau_spec_syntax", function()
  local src = [==[
const MAX_RETRIES: number = 3
const VERSION = "1.0"

const function greet(name: string): string
	return `hi {name}`
end

@native const function fast(x: number)
end

export type function serialize<T>(value: T): string
	return value
end

declare function warn<T...>(...: T...): ()

declare game: DataModel

declare extern type Instance extends RBXScriptSignal
	read Name: string
	Parent: Instance?
	function FindFirstChild(self, name: string): Instance?
end

declare class LegacyService
	AutoSave: boolean
	function Save(self): boolean
end

@native function step(dt: number)
end
]==]
  local out = idx(src, "luau")
  has(out, {
    "consts:",
    "MAX_RETRIES = 3",
    'VERSION = "1.0"',
    "game: DataModel",
    "types:",
    "export type function serialize<T>(value: T): string",
    "extern type Instance extends RBXScriptSignal",
    "fns:",
    "greet(name: string): string",
    "fast(x: number)",
    "warn<T...>(...: T...)",
    "step(dt: number)",
  })
end)
