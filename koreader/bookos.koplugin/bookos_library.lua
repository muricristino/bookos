local Blitbuffer = require("ffi/blitbuffer")
local Button = require("ui/widget/button")
local ButtonDialog = require("ui/widget/buttondialog")
local CenterContainer = require("ui/widget/container/centercontainer")
local Device = require("device")
local DocumentRegistry = require("document/documentregistry")
local DocSettings = require("docsettings")
local Font = require("ui/font")
local FrameContainer = require("ui/widget/container/framecontainer")
local Geom = require("ui/geometry")
local GestureRange = require("ui/gesturerange")
local HorizontalGroup = require("ui/widget/horizontalgroup")
local HorizontalSpan = require("ui/widget/horizontalspan")
local IconButton = require("ui/widget/iconbutton")
local ImageWidget = require("ui/widget/imagewidget")
local InfoMessage = require("ui/widget/infomessage")
local InputContainer = require("ui/widget/container/inputcontainer")
local JSON = require("json")
local LeftContainer = require("ui/widget/container/leftcontainer")
local LineWidget = require("ui/widget/linewidget")
local OverlapGroup = require("ui/widget/overlapgroup")
local RightContainer = require("ui/widget/container/rightcontainer")
local TopContainer = require("ui/widget/container/topcontainer")
local TextBoxWidget = require("ui/widget/textboxwidget")
local TextWidget = require("ui/widget/textwidget")
local UIManager = require("ui/uimanager")
local VerticalGroup = require("ui/widget/verticalgroup")
local VerticalSpan = require("ui/widget/verticalspan")
local lfs = require("libs/libkoreader-lfs")
local logger = require("logger")
local util = require("util")
local Screen = Device.screen

local BLACK = Blitbuffer.COLOR_BLACK
local GRAY = Blitbuffer.COLOR_DARK_GRAY
local LIGHT = Blitbuffer.COLOR_LIGHT_GRAY
local S = function(n) return Screen:scaleBySize(n) end

local BOOK_EXT = { epub = true, azw3 = true, azw = true, mobi = true, pdf = true, txt = true, fb2 = true, djvu = true, cbz = true }
local SKIP_FILES = { ["JAILBROKEN.txt"] = true, ["My Clippings.txt"] = true, ["spidercat.azw3"] = true }

local TABS = { "Recentes", "Meus livros", "Loja" }
local STORE_TITLES = {
    ["New arrivals"] = "Recém-chegados",
    ["Brazilian fiction"] = "Ficção brasileira",
    ["Fantasy"] = "Fantasia",
    ["Mystery and crime"] = "Policial e mistério",
    ["Russian classics"] = "Clássicos russos",
}

local deps
local state = { tab = 1, page = { 1, 1, 1 } }
local scan = { at = 0, books = nil }
local meta_cache = {}
local store = { at = 0, shelves = nil, loading = false, error = nil }
local pending_covers = {}

local M = {}
M.id = "bookos_library"
M.name = "bookos: Biblioteca"
M.label = nil
M.enabled_key = "bookos_library_enabled"
M.default_on = true
M.has_covers = true
M.is_book_mod = true

function M.setup(d)
    deps = d
end

local function face(name, size)
    local ok, f = pcall(Font.getFace, Font, name, size)
    if ok and f then return f end
    return Font:getFace("tfont", size)
end

local function serif(size, bold)
    return face(bold and "literata/Literata-Bold.ttf" or "literata/Literata-Regular.ttf", size)
end

local function sans(size)
    return Font:getFace("cfont", size)
end

local function trim(s)
    return (s or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function fold(s)
    s = (s or ""):lower()
    local map = { ["á"] = "a", ["à"] = "a", ["â"] = "a", ["ã"] = "a", ["ä"] = "a", ["é"] = "e", ["è"] = "e", ["ê"] = "e", ["ë"] = "e", ["í"] = "i", ["ì"] = "i", ["î"] = "i", ["ï"] = "i", ["ó"] = "o", ["ò"] = "o", ["ô"] = "o", ["õ"] = "o", ["ö"] = "o", ["ú"] = "u", ["ù"] = "u", ["û"] = "u", ["ü"] = "u", ["ç"] = "c", ["ñ"] = "n", ["ý"] = "y", ["Á"] = "a", ["É"] = "e", ["Í"] = "i", ["Ó"] = "o", ["Ú"] = "u", ["Ç"] = "c" }
    for k, v in pairs(map) do s = s:gsub(k, v) end
    return s
end

local function humanName(name)
    name = trim((name or ""):gsub("%[.-%]", ""):gsub("%s+_%s+.*$", ""))
    name = trim(name:gsub("%s*&.*$", ""):gsub("%s*;.*$", ""))
    local first_of_many = name:match("^([^,]+),%S")
    if first_of_many then name = first_of_many end
    name = trim(name:gsub("[,%s]+$", ""))
    name = name:gsub("_$", "."):gsub("_ ", ". ")
    local last, first = name:match("^([^,]+),%s*([^,]+)$")
    if last and first then name = trim(first) .. " " .. trim(last) end
    name = name:gsub("%f[%w](%u)(%u%u%u+)%f[%W]", function(a, b) return a .. b:lower() end)
    return trim(name)
end

local function unflip(title)
    for _, a in ipairs({ "A", "O", "As", "Os", "The", "Um", "Uma" }) do
        local rest = title:match("^(.*), " .. a .. "$")
        if rest then return a .. " " .. rest end
    end
    return title
end

local function cleanTitle(t)
    t = t:gsub("%s*%- libgen.*$", ""):gsub("_[A-Z0-9]+$", ""):gsub("%s*%(z%-lib%.org%)", ""):gsub("%s*%[[0-9a-f]+%]$", "")
    t = t:gsub("%s*%(%d%d%d%d[^)]*%)$", ""):gsub("%s*%(Portuguese Edition%)", "")
    return unflip(trim(t:gsub("_", ":")))
end

local function bim()
    local ok, Config = pcall(require, "infra/sui_config")
    if ok and Config and Config.getBookInfoManager then return Config.getBookInfoManager(), Config end
end

local NOT_AUTHOR_DIRS = { bookos = true, Downloads = true, Unknown = true, Items01 = true }

local function pathMeta(fp, home)
    local rel = fp:sub(#home + 2)
    local parts = util.splitToArray(rel, "/")
    local file = parts[#parts] or rel
    local stem = file:gsub("%.[^.]+$", "")
    local folder = #parts >= 2 and parts[#parts - 1] or ""
    local author, title
    local calibre_title, calibre_author = stem:match("^(.*) %- ([^-]+)$")
    local author_dir = #parts == 2 and not NOT_AUTHOR_DIRS[folder] and not folder:match("^[%w%-]+%.%a+$")
    local lead, rest = stem:match("^(.-) %- (.+)$")
    local lead_is_name = lead and not lead:find("%d") and select(2, lead:gsub("%S+", "")) <= 3
    if author_dir and calibre_author and humanName(calibre_author) == humanName(folder) then
        title, author = calibre_title, calibre_author
    elseif author_dir and folder:find(", ") and lead_is_name then
        title, author = rest, lead
    elseif author_dir and folder:find(", ") then
        title, author = calibre_title or stem, folder
    elseif folder == "Unknown" then
        title = stem:match("^(.*) %- Unknown$") or stem
    else
        local a, t = stem:match("^(.-) %- (.+)$")
        local words = function(x) return select(2, x:gsub("%S+", "")) end
        if a and words(a) > 4 and words(t) <= 4 and not t:find("%d") then
            author, title = t, a
        elseif a and not a:find("%d") then
            author, title = a, t
        else
            title = stem
        end
    end
    author = author and humanName(author) or nil
    if not author or author == "" or author == "Unknown" then
        local z = (title or stem):match("%(([^()]+)%) %(z%-lib%.org%)")
        author = z and humanName(z) or nil
    end
    return author, cleanTitle(title or stem)
end

local function bookMeta(fp, home)
    local cached = meta_cache[fp]
    if cached then return cached end
    local author, title
    local manager = bim()
    if manager then
        local ok, info = pcall(manager.getBookInfo, manager, fp, false)
        if ok and info then
            author = info.authors and info.authors ~= "" and humanName(info.authors:gsub("\n.*", "")) or nil
            title = info.title and info.title ~= "" and info.title or nil
        end
    end
    local p_author, p_title = pathMeta(fp, home)
    local m = { file = fp, author = author or p_author, title = title or p_title }
    if not m.author or m.author == "" or m.author == "Unknown" then m.author = "Autor desconhecido" end
    if author then meta_cache[fp] = m end
    return m
end

local function homeDir()
    return G_reader_settings:readSetting("home_dir") or "/mnt/us/documents"
end

local function scanLibrary()
    if scan.books and os.time() - scan.at < 60 then return scan.books end
    local home = homeDir()
    local books = {}
    local function walk(dir, depth)
        if depth > 4 then return end
        local ok, iter, obj = pcall(lfs.dir, dir)
        if not ok then return end
        for name in iter, obj do
            if name ~= "." and name ~= ".." and not name:match("^%.") then
                local path = dir .. "/" .. name
                local mode = lfs.attributes(path, "mode")
                if mode == "directory" then
                    if not name:match("%.sdr$") and name ~= "dictionaries" then walk(path, depth + 1) end
                elseif mode == "file" and not SKIP_FILES[name] then
                    local ext = (name:match("%.([^.]+)$") or ""):lower()
                    if BOOK_EXT[ext] then books[#books + 1] = path end
                end
            end
        end
    end
    walk(home, 1)
    scan.books, scan.at = books, os.time()
    return books
end

local function authorKey(name)
    local s = fold(name):gsub("[^%a]", ""):gsub("ph", "f"):gsub("w", "v"):gsub("[aeiouy]", "")
    return (s:gsub("(%a)%1+", "%1"))
end

local function accents(name)
    return select(2, name:gsub("[\195-\197]", ""))
end

local FORMAT_RANK = { epub = 1, pdf = 2, fb2 = 3, txt = 4 }

local function dedupe(books)
    local seen, list = {}, {}
    for _, m in ipairs(books) do
        local key = fold(m.title):gsub("%b()", ""):gsub("%b[]", ""):gsub("[^%w]", "")
        local rank = FORMAT_RANK[(m.file:match("%.([^.]+)$") or ""):lower()] or 9
        local prev = seen[key]
        if not prev then
            seen[key] = { i = #list + 1, rank = rank }
            list[#list + 1] = m
        elseif rank < prev.rank then
            list[prev.i], prev.rank = m, rank
        end
    end
    return list
end

local function authorsShelves()
    local home = homeDir()
    local groups, order = {}, {}
    for _, fp in ipairs(scanLibrary()) do
        local m = bookMeta(fp, home)
        local key = m.author == "Autor desconhecido" and "?" or authorKey(m.author)
        if key == "" then key = fold(m.author) end
        if not groups[key] then
            groups[key] = { key = key, names = {}, books = {} }
            order[#order + 1] = key
        end
        local g = groups[key]
        g.names[m.author] = (g.names[m.author] or 0) + 1
        table.insert(g.books, m)
    end
    local by_last = {}
    for _, key in ipairs(order) do
        local name = next(groups[key].names)
        local last = name:match("(%S+)$")
        if name:find(" ") and last then
            local k = authorKey(last)
            by_last[k] = by_last[k] == nil and key or false
        end
    end
    for _, key in ipairs(order) do
        local g = groups[key]
        local name = next(g.names)
        local target = not name:find(" ") and key ~= "?" and by_last[authorKey(name)]
        if target and target ~= key then
            local t = groups[target]
            for _, m in ipairs(g.books) do table.insert(t.books, m) end
            groups[key] = nil
        end
    end
    local list = {}
    for _, key in ipairs(order) do
        local g = groups[key]
        if g then
            local best, best_score
            for name, count in pairs(g.names) do
                local score = accents(name) * 1000 + count
                if not best or score > best_score or (score == best_score and name < best) then best, best_score = name, score end
            end
            g.author = best
            g.books = dedupe(g.books)
            for _, m in ipairs(g.books) do m.author = best end
            table.sort(g.books, function(a, b) return fold(a.title) < fold(b.title) end)
            list[#list + 1] = g
        end
    end
    table.sort(list, function(a, b)
        local ua, ub = a.key == "?", b.key == "?"
        if ua ~= ub then return ub end
        if #a.books ~= #b.books then return #a.books > #b.books end
        return fold(a.author) < fold(b.author)
    end)
    return list
end

local function sortMode()
    return G_reader_settings:readSetting("bookos_library_sort") == "title" and "title" or "author"
end

local function initial(title)
    local c = fold(title):match("%w")
    if not c or c:match("%d") then return "#" end
    return c:upper()
end

local function titleShelves(groups)
    local all = {}
    for _, g in ipairs(groups) do
        for _, m in ipairs(g.books) do all[#all + 1] = m end
    end
    table.sort(all, function(a, b) return fold(a.title) < fold(b.title) end)
    local list, by = {}, {}
    for _, m in ipairs(all) do
        local letter = initial(m.title)
        if not by[letter] then
            by[letter] = { author = letter, books = {} }
            list[#list + 1] = by[letter]
        end
        table.insert(by[letter].books, m)
    end
    table.sort(list, function(a, b)
        if (a.author == "#") ~= (b.author == "#") then return b.author == "#" end
        return a.author < b.author
    end)
    return list
end

local function recentBooks(limit)
    local ok, ReadHistory = pcall(require, "readhistory")
    if not ok then return {} end
    local home = homeDir()
    local list, seen = {}, {}
    for _, item in ipairs(ReadHistory.hist or {}) do
        if #list >= limit then break end
        local m = item.file and lfs.attributes(item.file, "mode") == "file" and bookMeta(item.file, home)
        local key = m and fold(m.title):gsub("%b()", ""):gsub("%b[]", ""):gsub("[^%w]", "")
        if m and not seen[key] then
            seen[key] = true
            local percent, printed = 0, nil
            local ok_ds, ds = pcall(DocSettings.open, DocSettings, item.file)
            if ok_ds and ds then
                percent = tonumber(ds:readSetting("percent_finished")) or 0
                printed = tonumber(ds:readSetting("bookos_physical_pages"))
                local summary = ds:readSetting("summary")
                if summary and summary.status == "complete" then percent = 1 end
            end
            list[#list + 1] = { file = item.file, author = m.author, title = m.title, percent = percent, printed = printed }
        end
    end
    return list
end

local function tappable(widget, callback, hold)
    local ic = InputContainer:new{ widget }
    ic.ges_events = { BookosTap = { GestureRange:new{ ges = "tap", range = function() return ic.dimen end } } }
    function ic:onBookosTap()
        callback()
        return true
    end
    if hold then
        ic.ges_events.BookosHold = { GestureRange:new{ ges = "hold", range = function() return ic.dimen end } }
        function ic:onBookosHold()
            hold()
            return true
        end
    end
    return ic
end

local function readable(fp)
    local ok, has = pcall(DocumentRegistry.hasProvider, DocumentRegistry, fp)
    return ok and has
end

local function placeholder(title, w, h, note)
    return CenterContainer:new{
        dimen = Geom:new{ w = w, h = h },
        TextBoxWidget:new{ text = (title or "") .. (note and ("\n\n" .. note) or ""), face = serif(w < S(90) and 8 or 12), width = w - S(12), alignment = "center", height = h - S(12), height_overflow_show_ellipsis = true },
    }
end

local function framed(inner, w, h)
    return FrameContainer:new{ padding = 0, margin = 0, bordersize = 1, color = BLACK, background = Blitbuffer.COLOR_WHITE, width = w, height = h, inner }
end

local function cachedInfo(manager, fp)
    if not manager then return nil end
    local ok, info = pcall(manager.getBookInfo, manager, fp, false)
    if ok then return info end
end

local extract = { at = 0 }

local function extractMissing()
    local manager, Config = bim()
    if not manager or os.time() - extract.at < 20 then return end
    if manager.isExtractingInBackground and manager:isExtractingInBackground() then return end
    extract.at = os.time()
    local files = {}
    for _, fp in ipairs(scanLibrary()) do
        local info = cachedInfo(manager, fp)
        if not (info and info.cover_fetched) then
            files[#files + 1] = { filepath = fp, cover_specs = { max_cover_w = S(180), max_cover_h = S(270) } }
        end
    end
    if #files > 0 then
        pcall(manager.extractInBackground, manager, files)
        if Config then Config.cover_extraction_pending = true end
    end
end

local function localCover(book, w, h)
    local manager, Config = bim()
    local info = cachedInfo(manager, book.file)
    if info and info.cover_fetched and info.has_cover and Config then
        local bb = Config.getStretchedCoverBB(book.file, w - 2, h - 2)
        if bb then
            return framed(ImageWidget:new{ image = bb, image_disposable = false, width = w - 2, height = h - 2 }, w, h)
        end
    end
    local holder = framed(placeholder(book.title, w - 2, h - 2, not readable(book.file) and "azw3 · Kindle" or nil), w, h)
    if not (info and info.cover_fetched) then
        pending_covers[#pending_covers + 1] = { file = book.file, holder = holder, w = w, h = h }
        if Config then Config.cover_extraction_pending = true end
    end
    return holder
end

local function storeCoverPath(book, w)
    return deps.cacheDir() .. "/" .. book.md5 .. "_" .. w .. ".jpg"
end

local function storeCover(book, w, h)
    local path = storeCoverPath(book, w - 2)
    if lfs.attributes(path, "mode") == "file" then
        return framed(ImageWidget:new{ file = path, width = w - 2, height = h - 2 }, w, h)
    end
    return framed(placeholder(book.title, w - 2, h - 2), w, h)
end

local function shelfLine(w)
    return LineWidget:new{ dimen = Geom:new{ w = w, h = S(1.4) }, background = BLACK }
end

local function coverRow(items, cols, w, cover_w, cover_h, make)
    local gap = cols > 1 and math.floor((w - cols * cover_w) / (cols - 1)) or 0
    local row = HorizontalGroup:new{ align = "bottom" }
    for i, item in ipairs(items) do
        if i > 1 then row[#row + 1] = HorizontalSpan:new{ width = gap } end
        row[#row + 1] = make(item, cover_w, cover_h)
    end
    return VerticalGroup:new{ align = "left", row, shelfLine(w) }
end

local function shelfHeader(title, right, w, on_right)
    local right_w = right and TextWidget:new{ text = right, face = sans(14) }:getSize().w or 0
    local left = TextWidget:new{ text = title, face = serif(17, true), max_width = w - right_w - S(16), fgcolor = BLACK }
    local right_widget = right and TextWidget:new{ text = right, face = sans(14), fgcolor = GRAY } or HorizontalSpan:new{ width = 0 }
    if right and on_right then right_widget = tappable(right_widget, on_right) end
    local h = math.max(left:getSize().h, S(30))
    return OverlapGroup:new{
        dimen = Geom:new{ w = w, h = h },
        LeftContainer:new{ dimen = Geom:new{ w = w, h = h }, left },
        RightContainer:new{ dimen = Geom:new{ w = w, h = h }, right_widget },
    }
end

local function chunk(list, n)
    local rows, row = {}, {}
    for _, item in ipairs(list) do
        row[#row + 1] = item
        if #row == n then rows[#rows + 1] = row; row = {} end
    end
    if #row > 0 then rows[#rows + 1] = row end
    return rows
end

local function progressBar(w, percent)
    local h = S(2.5)
    local fill = math.floor(w * math.max(0, math.min(1, percent or 0)))
    return OverlapGroup:new{
        dimen = Geom:new{ w = w, h = h },
        LineWidget:new{ dimen = Geom:new{ w = w, h = h }, background = LIGHT },
        LineWidget:new{ dimen = Geom:new{ w = math.max(fill, percent and percent > 0 and S(4) or 0), h = h }, background = BLACK },
    }
end

local function refresh(mode)
    local ctx = M._ctx
    if ctx and ctx.refresh_fn then
        ctx.refresh_fn()
        if ctx._screen_widget then UIManager:setDirty(ctx._screen_widget, mode or "ui") end
    end
end

local function openBook(fp)
    if not readable(fp) then
        UIManager:show(InfoMessage:new{ text = "Este livro está em azw3, formato que o KOReader não abre. Leia pelo leitor do Kindle ou baixe a versão epub pela Loja." })
        return
    end
    local ctx = M._ctx
    if ctx and ctx.open_fn then return ctx.open_fn(fp) end
    local ReaderUI = require("apps/reader/readerui")
    ReaderUI:showReader(fp)
end

local function showBook(book)
    deps.localDetails({ file = book.file, title = book.title, author = book.author, open = openBook })
end

local function recentBlocks(w, avail)
    local cols = 4
    local caption_h = 0
    local overhead = S(8) + S(5) + S(2) + S(18)
    local cover_w = math.floor((w - (cols - 1) * S(16)) / cols)
    local cover_h = math.min(math.floor(cover_w * 1.5), math.floor((avail - caption_h - 2 * overhead) / 2))
    cover_w = math.min(cover_w, math.floor(cover_h / 1.5))
    local books = recentBooks(16)
    local blocks = {}
    if #books == 0 then
        blocks[1] = CenterContainer:new{ dimen = Geom:new{ w = w, h = S(120) }, TextWidget:new{ text = "Nenhum livro aberto ainda.", face = sans(16), fgcolor = GRAY } }
        return blocks
    end
    for i, row in ipairs(chunk(books, cols)) do
        local block = VerticalGroup:new{ align = "left" }
        block[1] = coverRow(row, cols, w, cover_w, cover_h, function(book, cw, ch)
            local cover = localCover(book, cw, ch)
            local item = VerticalGroup:new{ align = "left", progressBar(cw, book.percent), VerticalSpan:new{ width = S(5) }, cover }
            return tappable(item, function() openBook(book.file) end, function() showBook(book) end)
        end)
        block[#block + 1] = VerticalSpan:new{ width = S(18) }
        blocks[#blocks + 1] = block
    end
    return blocks
end

local function sortToggle(w, summary)
    local mode = sortMode()
    local function option(label, value)
        local active = mode == value
        local text = TextWidget:new{ text = label:upper(), face = sans(13), bold = active, fgcolor = active and BLACK or GRAY }
        local cell = FrameContainer:new{
            padding = S(8), padding_bottom = S(4), margin = 0, bordersize = 0,
            VerticalGroup:new{ align = "center", text, VerticalSpan:new{ width = S(3) }, LineWidget:new{ dimen = Geom:new{ w = text:getSize().w, h = S(2) }, background = active and BLACK or Blitbuffer.COLOR_WHITE } },
        }
        return tappable(cell, function()
            if active then return end
            G_reader_settings:saveSetting("bookos_library_sort", value)
            state.page[2] = 1
            refresh("flashui")
        end)
    end
    local h = S(40)
    return OverlapGroup:new{
        dimen = Geom:new{ w = w, h = h },
        LeftContainer:new{ dimen = Geom:new{ w = w, h = h }, TextWidget:new{ text = summary, face = sans(15), fgcolor = GRAY } },
        RightContainer:new{ dimen = Geom:new{ w = w, h = h }, HorizontalGroup:new{ align = "center", option("Por autor", "author"), option("Título A–Z", "title") } },
    }
end

local function libraryBlocks(w, avail)
    local head_h = S(30)
    local overhead = head_h + S(6) + S(2) + S(8) + S(10)
    local cols = 4
    local cover_w = math.floor((w - (cols - 1) * S(16)) / cols)
    local cover_h = math.min(math.floor(cover_w * 1.5), math.floor((avail - S(52)) / 2) - overhead - S(2))
    cover_w = math.min(cover_w, math.floor(cover_h / 1.5))
    local groups = authorsShelves()
    local total = 0
    for _, g in ipairs(groups) do total = total + #g.books end
    local blocks = {}
    if total == 0 then
        blocks[1] = CenterContainer:new{ dimen = Geom:new{ w = w, h = S(120) }, TextWidget:new{ text = "Nenhum livro na pasta inicial.", face = sans(16), fgcolor = GRAY } }
        return blocks
    end
    local summary = string.format("%d livros · %d autores", total, #groups)
    blocks[1] = VerticalGroup:new{ align = "left", sortToggle(w, summary), VerticalSpan:new{ width = S(12) } }
    if sortMode() == "title" then groups = titleShelves(groups) end
    for _, group in ipairs(groups) do
        local n = #group.books
        local rows = chunk(group.books, cols)
        for r, row in ipairs(rows) do
            local block = VerticalGroup:new{ align = "left" }
            if r == 1 then
                block[1] = shelfHeader(group.author, n == 1 and "1 livro" or (n .. " livros"), w)
                block[2] = VerticalSpan:new{ width = S(6) }
                block.bookos_index = group.author
                block.bookos_count = n
            end
            block[#block + 1] = coverRow(row, cols, w, cover_w, cover_h, function(book, cw, ch)
                return tappable(localCover(book, cw, ch), function() openBook(book.file) end, function() showBook(book) end)
            end)
            block[#block + 1] = VerticalSpan:new{ width = r == #rows and S(18) or S(8) }
            blocks[#blocks + 1] = block
        end
    end
    return blocks
end

local function storeTitle(t)
    if STORE_TITLES[t] then return STORE_TITLES[t] end
    local who = t:match("^More by (.+)$")
    if who then return "Mais de " .. who end
    return t
end

local function storeCoverWidth(w)
    return math.floor((w - 4 * S(14)) / 5) - 2
end

local function loadStore(w)
    if store.loading then return end
    store.loading = true
    UIManager:scheduleIn(0.2, function()
        local server = deps.server()
        if server == "" then
            store.error = "Defina o endereço do servidor bookos no menu."
        else
            local cw = storeCoverWidth(w)
            local code, body = deps.request(server .. "/store?w=" .. cw, nil, 20, 40)
            local ok, data = pcall(JSON.decode, body or "")
            if code == 200 and ok and type(data) == "table" then
                store.shelves, store.at, store.error = data.shelves or {}, os.time(), nil
            else
                store.error = "Não consegui falar com o servidor bookos."
                logger.warn("bookos library: store", code, body)
            end
        end
        store.loading = false
        if state.tab == 3 then refresh("ui") end
    end)
end

local function fetchStoreCovers(books, w)
    if store.fetching then return end
    local server = deps.server()
    local missing = {}
    for _, book in ipairs(books) do
        local path = storeCoverPath(book, w)
        if type(book.cover_url) == "string" and lfs.attributes(path, "mode") ~= "file" and not book.bookos_no_cover then
            missing[#missing + 1] = book
        end
    end
    if #missing == 0 or server == "" then return end
    store.fetching = true
    UIManager:scheduleIn(0.3, function()
        for _, book in ipairs(missing) do
            local path = storeCoverPath(book, w)
            local c = deps.request(server .. book.cover_url .. "?w=" .. w, path .. ".part", 10, 20)
            if c == 200 then os.rename(path .. ".part", path) else os.remove(path .. ".part"); book.bookos_no_cover = true end
        end
        store.fetching = false
        if state.tab == 3 then refresh("ui") end
    end)
end

local function searchRow(w)
    local h = S(46)
    local icon = IconButton:new{ icon = "appbar.search", width = S(26), height = S(26), padding = 0, callback = function() deps.search() end }
    local row = FrameContainer:new{
        bordersize = S(1.2), color = BLACK, radius = S(6), padding = 0, margin = 0, width = w, height = h,
        LeftContainer:new{
            dimen = Geom:new{ w = w - 2 * S(1.2), h = h - 2 * S(1.2) },
            HorizontalGroup:new{ align = "center", HorizontalSpan:new{ width = S(12) }, icon, HorizontalSpan:new{ width = S(10) }, TextWidget:new{ text = "Título, autor ou ISBN", face = serif(16), fgcolor = GRAY } },
        },
    }
    return tappable(row, function() deps.search() end)
end

local function storeBlocks(w)
    local blocks = {}
    local top = VerticalGroup:new{ align = "left", searchRow(w), VerticalSpan:new{ width = S(6) }, TextWidget:new{ text = "Busca no Anna's Archive pelo bookos · só epub", face = sans(13), fgcolor = GRAY }, VerticalSpan:new{ width = S(14) } }
    blocks[1] = top
    if not store.shelves or os.time() - store.at > 600 then loadStore(w) end
    if not store.shelves then
        blocks[2] = CenterContainer:new{ dimen = Geom:new{ w = w, h = S(80) }, TextWidget:new{ text = store.error or "Carregando a loja…", face = sans(16), fgcolor = GRAY } }
        return blocks
    end
    local cover_w = storeCoverWidth(w) + 2
    local cover_h = math.floor(cover_w * 1.5)
    for _, shelf in ipairs(store.shelves) do
        local books = shelf.books or {}
        local title = storeTitle(shelf.title or shelf.id or "")
        local block = VerticalGroup:new{ align = "left" }
        if #books == 0 then
            block[1] = shelfHeader(title, "montando…", w)
            block[2] = VerticalSpan:new{ width = S(14) }
        else
            block[1] = shelfHeader(title, "ver todos (" .. #books .. ")", w, function() deps.showList(title, books) end)
            block[2] = VerticalSpan:new{ width = S(6) }
            local first = {}
            for i = 1, math.min(4, #books) do first[i] = books[i] end
            block[3] = coverRow(first, 4, w, cover_w, cover_h, function(book, cw, ch)
                return tappable(storeCover(book, cw, ch), function() deps.details(book) end)
            end)
            block[4] = VerticalSpan:new{ width = S(18) }
            block.bookos_books = first
        end
        blocks[#blocks + 1] = block
    end
    return blocks
end

local function paginate(blocks, avail)
    local pages, page, used = {}, {}, 0
    for _, b in ipairs(blocks) do
        local h = b:getSize().h
        if #page > 0 and used + h > avail then
            pages[#pages + 1] = page
            page, used = {}, 0
        end
        page[#page + 1] = b
        used = used + h
    end
    if #page > 0 then pages[#pages + 1] = page end
    return pages
end

local function header(w)
    local h = S(52)
    local title = TextWidget:new{ text = "Biblioteca", face = serif(26, true), fgcolor = BLACK }
    local search = IconButton:new{ icon = "appbar.search", width = S(30), height = S(30), padding = S(8), callback = function() deps.search() end }
    local menu
    menu = IconButton:new{ icon = "appbar.menu", width = S(30), height = S(30), padding = S(8), callback = function()
        local dialog
        dialog = ButtonDialog:new{ buttons = {
            { { text = "Atualizar biblioteca e loja", callback = function()
                UIManager:close(dialog)
                scan.books, meta_cache, store.shelves, store.at = nil, {}, nil, 0
                refresh("full")
            end } },
            { { text = "Servidor bookos", callback = function()
                UIManager:close(dialog)
                deps.editServer()
            end } },
        } }
        UIManager:show(dialog)
    end }
    return OverlapGroup:new{
        dimen = Geom:new{ w = w, h = h },
        LeftContainer:new{ dimen = Geom:new{ w = w, h = h }, title },
        RightContainer:new{ dimen = Geom:new{ w = w, h = h }, HorizontalGroup:new{ align = "center", search, menu } },
    }
end

local function tabBar(w)
    local h = S(44)
    local tab_w = math.floor(w / #TABS)
    local row = HorizontalGroup:new{ align = "top" }
    for i, label in ipairs(TABS) do
        local active = i == state.tab
        local text = TextWidget:new{ text = label:upper(), face = sans(14), bold = active, fgcolor = active and BLACK or GRAY }
        local cell = VerticalGroup:new{
            align = "center",
            CenterContainer:new{ dimen = Geom:new{ w = tab_w, h = h - S(3) }, text },
            LineWidget:new{ dimen = Geom:new{ w = tab_w, h = S(3) }, background = active and BLACK or Blitbuffer.COLOR_WHITE },
        }
        row[#row + 1] = tappable(cell, function()
            if state.tab == i then return end
            state.tab = i
            refresh("flashui")
        end)
    end
    return VerticalGroup:new{ align = "left", row, LineWidget:new{ dimen = Geom:new{ w = w, h = S(1) }, background = BLACK } }
end

local function authorIndex(pages)
    local Menu = require("ui/widget/menu")
    local items = {}
    for p, list in ipairs(pages) do
        for _, block in ipairs(list) do
            if block.bookos_index then
                items[#items + 1] = { text = block.bookos_index, mandatory = tostring(block.bookos_count), page = p }
            end
        end
    end
    table.sort(items, function(a, b) return fold(a.text) < fold(b.text) end)
    local menu
    for _, item in ipairs(items) do
        local target = item.page
        item.callback = function()
            UIManager:close(menu)
            state.page[2] = target
            refresh("full")
        end
    end
    menu = Menu:new{ title = sortMode() == "title" and "Letras" or "Autores", item_table = items, covers_fullscreen = true, is_borderless = true, is_popout = false, _navbar_skip_inject = true }
    UIManager:show(menu)
end

local function pager(w, page, pages, all_pages)
    local h = S(44)
    if pages <= 1 then return VerticalSpan:new{ width = h } end
    local function go(delta)
        local p = math.max(1, math.min(pages, page + delta))
        if p == page then return end
        state.page[state.tab] = p
        refresh("full")
    end
    local prev = Button:new{ text = "◁", bordersize = 0, enabled = page > 1, text_font_size = 20, width = S(60), callback = function() go(-1) end }
    local nxt = Button:new{ text = "▷", bordersize = 0, enabled = page < pages, text_font_size = 20, width = S(60), callback = function() go(1) end }
    local label = TextWidget:new{ text = page .. " / " .. pages, face = sans(16) }
    local middle = label
    if state.tab == 2 then
        middle = tappable(HorizontalGroup:new{ align = "center", label, HorizontalSpan:new{ width = S(10) }, TextWidget:new{ text = sortMode() == "title" and "· letras" or "· autores", face = sans(14), fgcolor = GRAY } }, function() authorIndex(all_pages) end)
    end
    return CenterContainer:new{
        dimen = Geom:new{ w = w, h = h },
        HorizontalGroup:new{ align = "center", prev, HorizontalSpan:new{ width = S(24) }, middle, HorizontalSpan:new{ width = S(24) }, nxt },
    }
end

function M.getHeight(ctx)
    local ok, UI = pcall(require, "infra/sui_core")
    local sw = ctx and ctx._screen_widget
    local avail = (sw and sw._layout_content_h) or (ok and UI.getContentHeight()) or Screen:getHeight()
    local gap = ok and UI.MOD_GAP or S(23)
    return math.max(S(200), avail - gap - S(10))
end

local function build(w, ctx)
    M._ctx = ctx
    pending_covers = {}
    local H = M.getHeight(ctx)
    local head = header(w)
    local tabs = tabBar(w)
    local top_gap = S(14)
    local pager_h = S(44)
    local avail = H - head:getSize().h - tabs:getSize().h - top_gap - pager_h
    local ok, blocks = pcall(function()
        if state.tab == 1 then return recentBlocks(w, avail) end
        if state.tab == 2 then return libraryBlocks(w, avail) end
        return storeBlocks(w)
    end)
    if not ok then
        logger.warn("bookos library:", blocks)
        blocks = { TextWidget:new{ text = "Erro ao montar a estante.", face = sans(16) } }
    end
    if #pending_covers > 0 then UIManager:nextTick(extractMissing) end
    local pages = paginate(blocks, avail)
    local page = math.max(1, math.min(#pages, state.page[state.tab] or 1))
    state.page[state.tab] = page
    if state.tab == 3 then
        local visible = {}
        for _, b in ipairs(pages[page] or {}) do
            for _, book in ipairs(b.bookos_books or {}) do visible[#visible + 1] = book end
        end
        fetchStoreCovers(visible, storeCoverWidth(w))
    end
    local body = VerticalGroup:new{ align = "left" }
    for _, b in ipairs(pages[page] or {}) do body[#body + 1] = b end
    local content = VerticalGroup:new{
        align = "left",
        head,
        tabs,
        VerticalSpan:new{ width = top_gap },
        FrameContainer:new{ padding = 0, margin = 0, bordersize = 0, width = w, height = avail, TopContainer:new{ dimen = Geom:new{ w = w, h = avail }, body } },
        pager(w, page, #pages, pages),
    }
    local root = InputContainer:new{ dimen = Geom:new{ w = w, h = H }, FrameContainer:new{ padding = 0, margin = 0, bordersize = 0, width = w, height = H, background = Blitbuffer.COLOR_WHITE, content } }
    root.ges_events = { BookosSwipe = { GestureRange:new{ ges = "swipe", range = function() return root.dimen end } } }
    function root:onBookosSwipe(_, ges)
        local dir = ges and ges.direction
        if dir ~= "west" and dir ~= "east" then return end
        local p = page + (dir == "west" and 1 or -1)
        if p < 1 or p > #pages then return true end
        state.page[state.tab] = p
        refresh("full")
        return true
    end
    return root
end

function M.build(w, ctx)
    local ok, res = xpcall(build, debug.traceback, w, ctx)
    if ok then return res end
    logger.warn("bookos library:", res)
end

function M.updateCovers()
    local manager, Config = bim()
    if not (manager and Config) then return true end
    extractMissing()
    local still = {}
    for _, slot in ipairs(pending_covers) do
        local info = cachedInfo(manager, slot.file)
        if info and info.cover_fetched then
            local bb = info.has_cover and Config.getStretchedCoverBB(slot.file, slot.w - 2, slot.h - 2)
            if bb then
                slot.holder[1] = ImageWidget:new{ image = bb, image_disposable = false, width = slot.w - 2, height = slot.h - 2 }
            end
        else
            still[#still + 1] = slot
        end
    end
    pending_covers = still
    return #still == 0
end

function M.getMenuItems()
    return nil
end

function M.invalidate()
    scan.books, scan.at = nil, 0
end

return M
