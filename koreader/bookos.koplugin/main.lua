local Blitbuffer = require("ffi/blitbuffer")
local Button = require("ui/widget/button")
local CenterContainer = require("ui/widget/container/centercontainer")
local ConfirmBox = require("ui/widget/confirmbox")
local DataStorage = require("datastorage")
local Device = require("device")
local Dispatcher = require("dispatcher")
local Font = require("ui/font")
local FrameContainer = require("ui/widget/container/framecontainer")
local Geom = require("ui/geometry")
local GestureRange = require("ui/gesturerange")
local HorizontalGroup = require("ui/widget/horizontalgroup")
local HorizontalSpan = require("ui/widget/horizontalspan")
local ImageWidget = require("ui/widget/imagewidget")
local InfoMessage = require("ui/widget/infomessage")
local InputContainer = require("ui/widget/container/inputcontainer")
local InputDialog = require("ui/widget/inputdialog")
local JSON = require("json")
local LeftContainer = require("ui/widget/container/leftcontainer")
local LineWidget = require("ui/widget/linewidget")
local NetworkMgr = require("ui/network/manager")
local Size = require("ui/size")
local ScrollTextWidget = require("ui/widget/scrolltextwidget")
local TextBoxWidget = require("ui/widget/textboxwidget")
local TextWidget = require("ui/widget/textwidget")
local TitleBar = require("ui/widget/titlebar")
local TopContainer = require("ui/widget/container/topcontainer")
local UIManager = require("ui/uimanager")
local VerticalGroup = require("ui/widget/verticalgroup")
local VerticalSpan = require("ui/widget/verticalspan")
local WidgetContainer = require("ui/widget/container/widgetcontainer")
local http = require("socket.http")
local lfs = require("libs/libkoreader-lfs")
local logger = require("logger")
local ltn12 = require("ltn12")
local md5 = require("ffi/sha2").md5
local socket = require("socket")
local socketutil = require("socketutil")
local util = require("util")
local Screen = Device.screen

local PT = {
    ["Search books"] = "Buscar livros",
    ["Search"] = "Buscar",
    ["Title, author or ISBN"] = "Título, autor ou ISBN",
    ["Cancel"] = "Cancelar",
    ["Searching…"] = "Buscando…",
    ["Loading covers…"] = "Carregando capas…",
    ["Nothing found."] = "Nada encontrado.",
    ["Could not reach the bookos server at %1. Is the computer on and on the same network?"] = "Não consegui falar com o servidor bookos em %1. O computador está ligado e na mesma rede?",
    ["The server answered: %1"] = "O servidor respondeu: %1",
    ["Download"] = "Baixar",
    ["Downloading…"] = "Baixando…",
    ["Waiting for the server…"] = "Esperando o servidor…",
    ["The download failed: %1"] = "O download falhou: %1",
    ["The file arrived damaged (md5 mismatch). Try again."] = "O arquivo chegou corrompido (md5 não bate). Tente de novo.",
    ["Saved to %1"] = "Salvo em %1",
    ["Server address"] = "Endereço do servidor",
    ["Language: %1"] = "Idioma: %1",
    ["Search by: %1"] = "Buscar por: %1",
    ["Portuguese"] = "Português",
    ["English"] = "Inglês",
    ["Spanish"] = "Espanhol",
    ["French"] = "Francês",
    ["Any"] = "Qualquer",
    ["Everything"] = "Tudo",
    ["Author"] = "Autor",
    ["Title"] = "Título",
    ["Results for “%1”"] = "Resultados para “%1”",
    ["Page %1 of %2"] = "Página %1 de %2",
    ["no cover"] = "sem capa",
    ["Loading…"] = "Carregando…",
    ["Description"] = "Sinopse",
    ["No description."] = "Sem sinopse.",
    ["alternative publisher"] = "Outra editora",
    ["alternative edition"] = "Outra edição",
    ["alternative title"] = "Outro título",
    ["alternative author"] = "Outro autor",
    ["alternative description"] = "Outra sinopse",
}

local function tr(s, ...)
    local lang = G_reader_settings:readSetting("language") or ""
    local out = lang:match("^pt") and PT[s] or s
    local args = { ... }
    return (out:gsub("%%(%d)", function(i) return tostring(args[tonumber(i)] or "") end))
end

local function server()
    return ((G_reader_settings:readSetting("bookos_server") or ""):gsub("/+$", ""))
end

local LANGS = { { "pt", "Portuguese" }, { "en", "English" }, { "es", "Spanish" }, { "fr", "French" }, { "", "Any" } }
local FIELDS = { { "", "Everything" }, { "author", "Author" }, { "title", "Title" } }

local function choice(list, key, default)
    local v = G_reader_settings:readSetting(key) or default
    for i, item in ipairs(list) do
        if item[1] == v then return i, item end
    end
    return 1, list[1]
end

local function cycle(list, key, default)
    local i = choice(list, key, default)
    G_reader_settings:saveSetting(key, list[i % #list + 1][1])
end

local function cacheDir()
    local dir = DataStorage:getDataDir() .. "/cache/bookos"
    util.makePath(dir)
    return dir
end

local function downloadDir()
    local home = G_reader_settings:readSetting("home_dir") or DataStorage:getDataDir()
    local dir = home .. "/bookos"
    util.makePath(dir)
    return dir
end

local function urlencode(s)
    return (s:gsub("[^%w%-_%.~]", function(c) return string.format("%%%02X", string.byte(c)) end))
end

local function urldecode(s)
    return (s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
end

local function request(url, file, block_timeout, total_timeout)
    local chunks = {}
    local sink
    local handle
    if file then
        handle = io.open(file, "wb")
        if not handle then return nil, "cannot write " .. file end
        sink = ltn12.sink.file(handle)
    else
        sink = ltn12.sink.table(chunks)
    end
    socketutil:set_timeout(block_timeout or 15, total_timeout or 60)
    local code, headers, status = socket.skip(1, http.request{ url = url, method = "GET", sink = sink })
    socketutil:reset_timeout()
    if headers == nil then
        if file then os.remove(file) end
        return nil, status or code
    end
    return code, table.concat(chunks), headers
end

local function fileMD5(path)
    local f = io.open(path, "rb")
    if not f then return nil end
    local update = md5()
    while true do
        local chunk = f:read(1024 * 1024)
        if not chunk then break end
        update(chunk)
    end
    f:close()
    return update()
end

local function infoLine(book)
    local parts = {}
    for _, v in ipairs({ book.year, book.lang, book.ext, book.size }) do
        if v and v ~= "" then parts[#parts + 1] = v end
    end
    return table.concat(parts, " · ")
end

local ResultRow = InputContainer:extend{
    book = nil,
    cover = nil,
    width = nil,
    height = nil,
    callback = nil,
}

function ResultRow:init()
    self.dimen = Geom:new{ x = 0, y = 0, w = self.width, h = self.height }
    local pad = Size.padding.large
    local cover_h = self.height - 2 * pad
    local cover_w = math.floor(cover_h * 2 / 3)
    local cover
    if self.cover then
        cover = ImageWidget:new{ file = self.cover, width = cover_w, height = cover_h, scale_factor = 0 }
    else
        cover = FrameContainer:new{
            width = cover_w, height = cover_h, padding = 0, bordersize = Size.border.thin,
            CenterContainer:new{
                dimen = Geom:new{ w = cover_w - 2 * Size.border.thin, h = cover_h - 2 * Size.border.thin },
                TextWidget:new{ text = tr("no cover"), face = Font:getFace("cfont", 14), fgcolor = Blitbuffer.COLOR_DARK_GRAY },
            },
        }
    end
    local text_w = self.width - cover_w - 3 * pad
    local text = VerticalGroup:new{
        align = "left",
        TextBoxWidget:new{ text = self.book.title or "", face = Font:getFace("tfont", 20), bold = true, width = text_w, height_adjust = true, height = math.floor(cover_h * 0.55), height_overflow_show_ellipsis = true },
        VerticalSpan:new{ width = Size.span.vertical_default },
        TextWidget:new{ text = self.book.author or "", face = Font:getFace("cfont", 17), max_width = text_w },
        VerticalSpan:new{ width = Size.span.vertical_default },
        TextWidget:new{ text = infoLine(self.book), face = Font:getFace("cfont", 15), fgcolor = Blitbuffer.COLOR_DARK_GRAY, max_width = text_w },
    }
    self[1] = FrameContainer:new{
        padding = pad, bordersize = 0, background = Blitbuffer.COLOR_WHITE,
        HorizontalGroup:new{
            align = "center",
            CenterContainer:new{ dimen = Geom:new{ w = cover_w, h = cover_h }, cover },
            HorizontalSpan:new{ width = pad },
            LeftContainer:new{ dimen = Geom:new{ w = text_w, h = cover_h }, text },
        },
    }
    self.ges_events.Tap = { GestureRange:new{ ges = "tap", range = self.dimen } }
end

function ResultRow:onTap()
    if self.callback then self.callback(self.book) end
    return true
end

local Details = InputContainer:extend{
    book = nil,
    info = nil,
    cover = nil,
    plugin = nil,
}

function Details:init()
    self.width = Screen:getWidth()
    self.height = Screen:getHeight()
    self.dimen = Geom:new{ x = 0, y = 0, w = self.width, h = self.height }
    local book, info = self.book, self.info or {}
    local pad = Size.padding.large * 2
    local title_bar = TitleBar:new{
        title = "",
        width = self.width,
        with_bottom_line = false,
        close_callback = function() self:onClose() end,
        show_parent = self,
    }
    local cover_w = math.floor(self.width * 0.36)
    local cover_h = math.floor(cover_w * 3 / 2)
    local cover
    if self.cover then
        cover = ImageWidget:new{ file = self.cover, width = cover_w, height = cover_h, scale_factor = 0 }
    else
        cover = FrameContainer:new{
            width = cover_w, height = cover_h, padding = 0, bordersize = Size.border.thin,
            CenterContainer:new{
                dimen = Geom:new{ w = cover_w - 2 * Size.border.thin, h = cover_h - 2 * Size.border.thin },
                TextWidget:new{ text = tr("no cover"), face = Font:getFace("cfont", 16), fgcolor = Blitbuffer.COLOR_DARK_GRAY },
            },
        }
    end
    local text_w = self.width - cover_w - 3 * pad
    local side = VerticalGroup:new{
        align = "left",
        TextBoxWidget:new{ text = info.title or book.title or "", face = Font:getFace("tfont", 24), bold = true, width = text_w, height = math.floor(cover_h * 0.5), height_adjust = true, height_overflow_show_ellipsis = true },
        VerticalSpan:new{ width = Size.span.vertical_large * 2 },
        TextBoxWidget:new{ text = info.author or book.author or "", face = Font:getFace("cfont", 19), width = text_w, height = math.floor(cover_h * 0.18), height_adjust = true, height_overflow_show_ellipsis = true },
        VerticalSpan:new{ width = Size.span.vertical_default },
        TextWidget:new{ text = info.publisher or book.publisher or "", face = Font:getFace("cfont", 16), fgcolor = Blitbuffer.COLOR_DARK_GRAY, max_width = text_w },
        VerticalSpan:new{ width = Size.span.vertical_default },
        TextWidget:new{ text = infoLine(book), face = Font:getFace("cfont", 16), fgcolor = Blitbuffer.COLOR_DARK_GRAY, max_width = text_w },
    }
    local top = HorizontalGroup:new{
        align = "top",
        HorizontalSpan:new{ width = pad },
        cover,
        HorizontalSpan:new{ width = pad },
        side,
    }
    local download = Button:new{
        text = tr("Download"),
        width = self.width - 2 * pad,
        bordersize = Size.border.thick,
        radius = Size.radius.button,
        text_font_size = 22,
        text_font_bold = true,
        padding = Size.padding.large,
        callback = function() NetworkMgr:runWhenOnline(function() self.plugin:download(book) end) end,
        show_parent = self,
    }
    local lines = {}
    local description = info.description
    for _, f in ipairs(info.fields or {}) do
        if (description == nil or description == "") and f[1] == "alternative description" then
            description = f[2]
        else
            local label = tr(f[1])
            lines[#lines + 1] = label:sub(1, 1):upper() .. label:sub(2) .. ": " .. f[2]
        end
    end
    local body = (description and description ~= "") and description or tr("No description.")
    if #lines > 0 then body = body .. "\n\n" .. table.concat(lines, "\n") end
    local heading = TextWidget:new{ text = tr("Description"), face = Font:getFace("tfont", 20), bold = true }
    local used = title_bar:getHeight() + cover_h + heading:getSize().h + download:getSize().h + 6 * Size.span.vertical_large + 2 * pad
    local text = ScrollTextWidget:new{
        text = body,
        face = Font:getFace("cfont", 18),
        width = self.width - 2 * pad,
        height = math.max(Screen:scaleBySize(80), self.height - used),
        dialog = self,
    }
    self[1] = FrameContainer:new{
        width = self.width, height = self.height, padding = 0, bordersize = 0, background = Blitbuffer.COLOR_WHITE,
        VerticalGroup:new{
            align = "left",
            title_bar,
            top,
            VerticalSpan:new{ width = Size.span.vertical_large * 3 },
            HorizontalGroup:new{ HorizontalSpan:new{ width = pad }, heading },
            VerticalSpan:new{ width = Size.span.vertical_large },
            HorizontalGroup:new{ HorizontalSpan:new{ width = pad }, text },
            VerticalSpan:new{ width = Size.span.vertical_large * 2 },
            HorizontalGroup:new{ HorizontalSpan:new{ width = pad }, download },
        },
    }
    if Device:isTouchDevice() then
        self.ges_events.Swipe = { GestureRange:new{ ges = "swipe", range = self.dimen } }
    end
end

function Details:onSwipe(_, ges)
    if ges.direction == "south" or ges.direction == "east" then
        return self:onClose()
    end
end

function Details:onClose()
    UIManager:close(self, "flashui")
    return true
end

function Details:onShow()
    UIManager:setDirty(self, "flashui")
end

local Results = InputContainer:extend{
    query = "",
    books = nil,
    plugin = nil,
    page = 1,
}

function Results:init()
    self.width = Screen:getWidth()
    self.height = Screen:getHeight()
    self.dimen = Geom:new{ x = 0, y = 0, w = self.width, h = self.height }
    self.title_bar = TitleBar:new{
        title = tr("Results for “%1”", self.query),
        width = self.width,
        with_bottom_line = true,
        close_callback = function() self:onClose() end,
        show_parent = self,
    }
    self.footer_h = Screen:scaleBySize(56)
    self.row_h = Screen:scaleBySize(128)
    self.per_page = math.max(1, math.floor((self.height - self.title_bar:getHeight() - self.footer_h) / (self.row_h + Size.line.thin)))
    self:countPages()
    if Device:isTouchDevice() then
        self.ges_events.Swipe = { GestureRange:new{ ges = "swipe", range = self.dimen } }
    end
    self:render()
end

function Results:countPages()
    self.pages = math.max(1, math.ceil(#self.books / self.per_page))
end

function Results:render()
    local first = (self.page - 1) * self.per_page + 1
    local last = math.min(#self.books, first + self.per_page - 1)
    local list = VerticalGroup:new{ align = "left" }
    for i = first, last do
        local book = self.books[i]
        list[#list + 1] = ResultRow:new{
            book = book,
            cover = self.plugin:coverPath(book),
            width = self.width,
            height = self.row_h,
            callback = function(b) self.plugin:showDetails(b) end,
        }
        list[#list + 1] = LineWidget:new{ dimen = Geom:new{ w = self.width, h = Size.line.thin }, background = Blitbuffer.COLOR_LIGHT_GRAY }
    end
    local prev = Button:new{ text = "◁", bordersize = 0, enabled = self.page > 1, callback = function() self:goTo(self.page - 1) end, show_parent = self }
    local nxt = Button:new{ text = "▷", bordersize = 0, enabled = self.page < self.pages or self.more, callback = function() self:goTo(self.page + 1) end, show_parent = self }
    local label = TextWidget:new{ text = tr("Page %1 of %2", self.page, self.pages .. (self.more and "+" or "")), face = Font:getFace("cfont", 18) }
    local footer = CenterContainer:new{
        dimen = Geom:new{ w = self.width, h = self.footer_h },
        HorizontalGroup:new{ align = "center", prev, HorizontalSpan:new{ width = Size.span.horizontal_default * 6 }, label, HorizontalSpan:new{ width = Size.span.horizontal_default * 6 }, nxt },
    }
    local body_h = self.height - self.title_bar:getHeight() - self.footer_h
    self[1] = FrameContainer:new{
        width = self.width, height = self.height, padding = 0, bordersize = 0, background = Blitbuffer.COLOR_WHITE,
        VerticalGroup:new{
            align = "left",
            self.title_bar,
            TopContainer:new{ dimen = Geom:new{ w = self.width, h = body_h }, list },
            footer,
        },
    }
end

function Results:goTo(page)
    if page > self.pages and self.more then
        return self.plugin:loadMore(self, function() self:goTo(page) end)
    end
    if page < 1 or page > self.pages or page == self.page then return end
    self.page = page
    self.plugin:fetchCovers(self.books, (page - 1) * self.per_page + 1, page * self.per_page, function()
        self:render()
        UIManager:setDirty(self, "ui")
    end)
end

function Results:onSwipe(_, ges)
    if ges.direction == "west" then
        self:goTo(self.page + 1)
        return true
    elseif ges.direction == "east" then
        self:goTo(self.page - 1)
        return true
    elseif ges.direction == "south" then
        self:onClose()
        return true
    end
end

function Results:onClose()
    UIManager:close(self, "flashui")
    return true
end

function Results:onShow()
    UIManager:setDirty(self, "flashui")
end

local Bookos = WidgetContainer:extend{
    name = "bookos",
    is_doc_only = false,
}

local current

function Bookos:init()
    current = self
    Dispatcher:registerAction("bookos_search", { category = "none", event = "BookosSearch", title = tr("Search books"), general = true })
    self.ui.menu:registerToMainMenu(self)
    self:registerQuickAction()
end

function Bookos:registerQuickAction()
    local ok, QA = pcall(require, "features/sui_quickactions")
    if not ok or type(QA) ~= "table" or type(QA.register) ~= "function" then return end
    QA.register{
        id = "bookos_search",
        label = tr("Search"),
        icon = "resources/icons/mdlight/appbar.search.svg",
        is_in_place = true,
        is_async_in_place = true,
        execute = function() (current or self):search() end,
    }
end

function Bookos:onBookosSearch()
    self:search()
    return true
end

function Bookos:addToMainMenu(menu_items)
    menu_items.bookos = {
        text = "bookos",
        sorting_hint = "search",
        sub_item_table = {
            { text = tr("Search books"), callback = function() self:search() end },
            {
                text_func = function() return tr("Server address") .. ": " .. (server() ~= "" and server() or "—") end,
                keep_menu_open = true,
                callback = function(menu) self:editServer(menu) end,
            },
        },
    }
end

function Bookos:editServer(menu, after)
    local dialog
    dialog = InputDialog:new{
        title = tr("Server address"),
        input = server() ~= "" and server() or "http://",
        buttons = { {
            { text = tr("Cancel"), id = "close", callback = function() UIManager:close(dialog) end },
            { text = "OK", is_enter_default = true, callback = function()
                local v = dialog:getInputText():gsub("%s", "")
                if v ~= "" and v ~= "http://" then G_reader_settings:saveSetting("bookos_server", v) end
                UIManager:close(dialog)
                if menu then menu:updateItems() end
                if after and server() ~= "" then after() end
            end },
        } },
    }
    UIManager:show(dialog)
    dialog:onShowKeyboard()
end

function Bookos:search()
    if server() == "" then
        return self:editServer(nil, function() self:search() end)
    end
    local dialog
    local function langText() return tr("Language: %1", tr(select(2, choice(LANGS, "bookos_lang", "pt"))[2])) end
    local function fieldText() return tr("Search by: %1", tr(select(2, choice(FIELDS, "bookos_by", ""))[2])) end
    local function toggle(id, list, key, default, text)
        cycle(list, key, default)
        local button = dialog.button_table:getButtonById(id)
        button:setText(text(), button.width)
        UIManager:setDirty(dialog, "ui")
    end
    dialog = InputDialog:new{
        title = tr("Search books"),
        input_hint = tr("Title, author or ISBN"),
        buttons = { {
            { text = langText(), id = "lang", callback = function() toggle("lang", LANGS, "bookos_lang", "pt", langText) end },
            { text = fieldText(), id = "by", callback = function() toggle("by", FIELDS, "bookos_by", "", fieldText) end },
        }, {
            { text = tr("Cancel"), id = "close", callback = function() UIManager:close(dialog) end },
            { text = tr("Search"), is_enter_default = true, callback = function()
                local q = dialog:getInputText()
                UIManager:close(dialog)
                if q and q:match("%S") then
                    NetworkMgr:runWhenOnline(function() self:runSearch(q) end)
                end
            end },
        } },
    }
    UIManager:show(dialog)
    dialog:onShowKeyboard()
end

function Bookos:serverError(err)
    UIManager:show(InfoMessage:new{ text = tr("Could not reach the bookos server at %1. Is the computer on and on the same network?", server()) })
    logger.warn("bookos:", err)
end

function Bookos:runSearch(q)
    local msg = InfoMessage:new{ text = tr("Searching…") }
    UIManager:show(msg)
    UIManager:forceRePaint()
    local params = {
        q = q,
        lang = select(2, choice(LANGS, "bookos_lang", "pt"))[1],
        by = select(2, choice(FIELDS, "bookos_by", ""))[1],
    }
    local books = self:fetchPage(params, 1)
    UIManager:close(msg)
    if not books then return end
    if #books == 0 then
        UIManager:show(InfoMessage:new{ text = tr("Nothing found.") })
        return
    end
    local results = Results:new{ query = q, books = books, plugin = self, params = params, aa_page = 1, more = #books > 0 }
    self:fetchCovers(books, 1, results.per_page, function()
        results:render()
        UIManager:show(results)
    end)
end

function Bookos:fetchPage(params, page)
    local url = server() .. "/search?format=epub&n=100&w=" .. Screen:scaleBySize(110) .. "&page=" .. page .. "&q=" .. urlencode(params.q)
    if params.lang ~= "" then url = url .. "&lang=" .. params.lang end
    if params.by ~= "" then url = url .. "&by=" .. params.by end
    local code, body = request(url, nil, 120, 150)
    if not code then
        self:serverError(body)
        return
    end
    local ok, data = pcall(JSON.decode, body)
    if code ~= 200 or not ok or type(data) ~= "table" then
        UIManager:show(InfoMessage:new{ text = tr("The server answered: %1", ok and type(data) == "table" and data.error or tostring(code)) })
        return
    end
    return data.results or {}
end

function Bookos:loadMore(results, done)
    local msg = InfoMessage:new{ text = tr("Searching…") }
    UIManager:show(msg)
    UIManager:forceRePaint()
    local books = self:fetchPage(results.params, results.aa_page + 1)
    UIManager:close(msg)
    if not books then return end
    results.aa_page = results.aa_page + 1
    local seen = {}
    for _, b in ipairs(results.books) do seen[b.md5] = true end
    local added = 0
    for _, b in ipairs(books) do
        if not seen[b.md5] then
            results.books[#results.books + 1] = b
            added = added + 1
        end
    end
    results.more = added > 0
    results:countPages()
    if added > 0 then return done() end
    results:render()
    UIManager:setDirty(results, "ui")
end

function Bookos:coverPath(book)
    local path = cacheDir() .. "/" .. book.md5 .. ".jpg"
    if lfs.attributes(path, "mode") == "file" then return path end
end

function Bookos:fetchCovers(books, first, last, done)
    local missing = {}
    for i = first, math.min(last, #books) do
        local b = books[i]
        if type(b.cover_url) == "string" and not self:coverPath(b) then missing[#missing + 1] = b end
    end
    if #missing == 0 then return done() end
    local msg = InfoMessage:new{ text = tr("Loading covers…") }
    UIManager:show(msg)
    UIManager:forceRePaint()
    for _, b in ipairs(missing) do
        local path = cacheDir() .. "/" .. b.md5 .. ".jpg"
        local code = request(server() .. b.cover_url .. "?w=" .. Screen:scaleBySize(110), path .. ".part", 20, 40)
        if code == 200 then
            os.rename(path .. ".part", path)
        else
            os.remove(path .. ".part")
        end
    end
    UIManager:close(msg)
    done()
end

function Bookos:showDetails(book)
    local msg = InfoMessage:new{ text = tr("Loading…") }
    UIManager:show(msg)
    UIManager:forceRePaint()
    local info
    local code, body = request(server() .. "/book/" .. book.md5, nil, 60, 90)
    if code == 200 then
        local ok, data = pcall(JSON.decode, body)
        if ok and type(data) == "table" then info = data end
    elseif not code then
        UIManager:close(msg)
        return self:serverError(body)
    end
    local cover
    local url = (info and info.cover_url) or book.cover_url
    if type(url) == "string" then
        local width = math.floor(Screen:getWidth() * 0.36)
        cover = cacheDir() .. "/" .. book.md5 .. "_" .. width .. ".jpg"
        if lfs.attributes(cover, "mode") ~= "file" then
            local c = request(server() .. url .. "?w=" .. width, cover .. ".part", 20, 40)
            if c == 200 then
                os.rename(cover .. ".part", cover)
            else
                os.remove(cover .. ".part")
                cover = self:coverPath(book)
            end
        end
    end
    UIManager:close(msg)
    UIManager:show(Details:new{ book = book, info = info, cover = cover, plugin = self })
end

function Bookos:download(book)
    local tmp = cacheDir() .. "/" .. book.md5 .. ".download"
    local msg
    local function show(text)
        if msg then UIManager:close(msg) end
        msg = InfoMessage:new{ text = text, dismissable = false }
        UIManager:show(msg)
        UIManager:forceRePaint()
    end
    local function finish(text)
        if msg then UIManager:close(msg) end
        if text then UIManager:show(InfoMessage:new{ text = text }) end
    end
    local poll
    poll = function()
        local code, err, headers = request(server() .. "/download/" .. book.md5, tmp, 30, 900)
        if not code then
            finish()
            return self:serverError(err)
        end
        if code == 202 then
            local f = io.open(tmp, "rb")
            local body = f and f:read("*a") or ""
            if f then f:close() end
            os.remove(tmp)
            local ok, data = pcall(JSON.decode, body)
            local bytes = ok and type(data) == "table" and tonumber(data.bytes) or 0
            local size = ok and type(data) == "table" and data.size or ""
            local progress = bytes > 0 and string.format("%.1f MB / %s", bytes / 1048576, size ~= "" and size or "?") or tr("Waiting for the server…")
            show(tr("Downloading…") .. "\n\n" .. (book.title or "") .. "\n" .. progress)
            UIManager:scheduleIn(3, poll)
            return
        end
        if code ~= 200 then
            local f = io.open(tmp, "rb")
            local body = f and f:read("*a") or ""
            if f then f:close() end
            os.remove(tmp)
            local ok, data = pcall(JSON.decode, body)
            return finish(tr("The download failed: %1", ok and type(data) == "table" and data.error or tostring(code)))
        end
        if fileMD5(tmp) ~= book.md5 then
            os.remove(tmp)
            return finish(tr("The file arrived damaged (md5 mismatch). Try again."))
        end
        local disposition = headers and headers["content-disposition"] or ""
        local name = disposition:match("filename%*=UTF%-8''([^;]+)")
        name = name and urldecode(name) or (book.md5 .. "." .. (book.ext or "bin"))
        name = name:gsub("[/\\:*?\"<>|]", " ")
        local dest = downloadDir() .. "/" .. name
        os.remove(dest)
        os.rename(tmp, dest)
        finish()
        UIManager:show(InfoMessage:new{ text = tr("Saved to %1", dest), timeout = 2 })
        UIManager:scheduleIn(1, function()
            local ReaderUI = require("apps/reader/readerui")
            ReaderUI:showReader(dest)
        end)
    end
    show(tr("Downloading…") .. "\n\n" .. (book.title or ""))
    UIManager:nextTick(poll)
end

return Bookos
