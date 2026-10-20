"""The Firefox Profiler fields used by the analysis and symbolication tools.

Profiles contain more fields; these types describe the tables samply writes that we read.
"""

from typing import NotRequired, TypedDict


class Library(TypedDict, total=False):
    name: str
    path: str


class Meta(TypedDict, total=False):
    product: str


class FuncTable(TypedDict):
    name: list[int]
    resource: list[int | None]


class FrameTable(TypedDict):
    func: list[int]


class StackTable(TypedDict):
    prefix: list[int | None]
    frame: list[int]


class Samples(TypedDict):
    stack: list[int | None]
    length: int


class ResourceTable(TypedDict):
    lib: list[int | None]


class Thread(TypedDict):
    name: NotRequired[str]
    processName: NotRequired[str]
    stringArray: list[str]
    funcTable: FuncTable
    frameTable: FrameTable
    stackTable: StackTable
    samples: Samples
    resourceTable: ResourceTable


class Profile(TypedDict):
    meta: NotRequired[Meta]
    libs: NotRequired[list[Library]]
    threads: list[Thread]
