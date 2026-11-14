"""Fictional. Copies pages/ to out/ and changes nothing else."""
import pathlib
import shutil

out = pathlib.Path("out")
shutil.rmtree(out, ignore_errors=True)
shutil.copytree("pages", out)
